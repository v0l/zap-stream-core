use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::mem::transmute;
use std::ops::Sub;
use std::time::Instant;

use crate::egress::hls::HlsEgress;
use crate::egress::recorder::RecorderEgress;
use crate::egress::Egress;
use crate::ingress::ConnectionInfo;
use crate::pipeline::{EgressType, PipelineConfig};
use crate::variant::{StreamMapping, VariantStream};
use crate::webhook::Webhook;
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_free, av_get_sample_fmt, av_packet_free, av_rescale_q,
};
use ffmpeg_rs_raw::{
    cstr, get_frame_from_hw, Decoder, Demuxer, DemuxerInfo, Encoder, Resample, Scaler,
};
use itertools::Itertools;
use log::{info, warn};
use uuid::Uuid;

/// Pipeline runner is the main entry process for stream transcoding
/// Each client connection spawns a new [PipelineRunner] and it should be run in its own thread
/// using [ingress::spawn_pipeline]
pub struct PipelineRunner {
    connection: ConnectionInfo,

    /// Configuration for this pipeline (variants, egress config etc.)
    config: PipelineConfig,

    /// Singleton demuxer for this input
    demuxer: Demuxer,

    /// Singleton decoder for all stream
    decoder: Decoder,

    /// Scaler for a variant (variant_id, Scaler)
    scalers: HashMap<Uuid, Scaler>,

    /// Resampler for a variant (variant_id, Resample)
    resampler: HashMap<Uuid, Resample>,

    /// Encoder for a variant (variant_id, Encoder)
    encoders: HashMap<Uuid, Encoder>,

    /// Simple mapping to copy streams
    copy_stream: HashMap<Uuid, Uuid>,

    /// All configured egress'
    egress: Vec<Box<dyn Egress>>,

    fps_counter_start: Instant,
    frame_ctr: u64,
    webhook: Webhook,

    info: Option<DemuxerInfo>,
}

impl PipelineRunner {
    pub fn new(
        connection: ConnectionInfo,
        webhook: Webhook,
        recv: Box<dyn Read + Send>,
    ) -> Result<Self> {
        Ok(Self {
            connection,
            config: Default::default(),
            demuxer: Demuxer::new_custom_io(recv, None)?,
            decoder: Decoder::new(),
            scalers: Default::default(),
            resampler: Default::default(),
            encoders: Default::default(),
            copy_stream: Default::default(),
            fps_counter_start: Instant::now(),
            egress: Vec::new(),
            frame_ctr: 0,
            webhook,
            info: None,
        })
    }

    /// Main processor, should be called in a loop
    pub unsafe fn run(&mut self) -> Result<()> {
        self.setup()?;

        // run transcoder pipeline
        let (mut pkt, stream) = self.demuxer.get_packet()?;
        let src_index = (*stream).index;

        // TODO: For copy streams, skip decoder
        let frames = if let Ok(frames) = self.decoder.decode_pkt(pkt) {
            frames
        } else {
            warn!("Error decoding frames");
            return Ok(());
        };

        for frame in frames {
            self.frame_ctr += 1;

            // Copy frame from GPU if using hwaccel decoding
            let mut frame = get_frame_from_hw(frame)?;
            (*frame).time_base = (*stream).time_base;

            // Get the variants which want this pkt
            let pkt_vars = self
                .config
                .variants
                .iter()
                .filter(|v| v.src_index() == src_index as usize);
            for var in pkt_vars {
                let enc = if let Some(enc) = self.encoders.get_mut(&var.id()) {
                    enc
                } else {
                    //warn!("Frame had nowhere to go in {} :/", var.id());
                    continue;
                };

                let mut new_frame = false;
                let mut frame = match var {
                    VariantStream::Video(v) => {
                        if let Some(s) = self.scalers.get_mut(&v.id()) {
                            new_frame = true;
                            s.process_frame(frame, v.width, v.height, transmute(v.pixel_format))?
                        } else {
                            frame
                        }
                    }
                    VariantStream::Audio(a) => {
                        if let Some(r) = self.resampler.get_mut(&a.id()) {
                            let frame_size = (*enc.codec_context()).frame_size;
                            // TODO: resample audio fifo
                            new_frame = true;
                            r.process_frame(frame, frame_size)?
                        } else {
                            frame
                        }
                    }
                    _ => frame,
                };

                // before encoding frame, rescale timestamps
                if !frame.is_null() {
                    let enc_ctx = enc.codec_context();
                    (*frame).pts =
                        av_rescale_q((*frame).pts, (*frame).time_base, (*enc_ctx).time_base);
                    (*frame).pkt_dts =
                        av_rescale_q((*frame).pkt_dts, (*frame).time_base, (*enc_ctx).time_base);
                    (*frame).duration =
                        av_rescale_q((*frame).duration, (*frame).time_base, (*enc_ctx).time_base);
                    (*frame).time_base = (*enc_ctx).time_base;
                }

                let packets = enc.encode_frame(frame)?;
                // pass new packets to egress
                for mut pkt in packets {
                    for eg in self.egress.iter_mut() {
                        eg.process_pkt(pkt, &var.id())?;
                    }
                    av_packet_free(&mut pkt);
                }

                if new_frame {
                    av_frame_free(&mut frame);
                }
            }

            av_frame_free(&mut frame);
        }

        av_packet_free(&mut pkt);

        let elapsed = Instant::now().sub(self.fps_counter_start).as_secs_f32();
        if elapsed >= 2f32 {
            info!("Average fps: {:.2}", self.frame_ctr as f32 / elapsed);
            self.fps_counter_start = Instant::now();
            self.frame_ctr = 0;
        }
        Ok(())
    }

    unsafe fn setup(&mut self) -> Result<()> {
        if self.info.is_some() {
            return Ok(());
        }

        let info = self.demuxer.probe_input()?;
        self.setup_pipeline(&info)?;
        self.info = Some(info);
        Ok(())
    }

    unsafe fn setup_pipeline(&mut self, info: &DemuxerInfo) -> Result<()> {
        let cfg = self.webhook.start(info);
        self.config = cfg.clone();

        // src stream indexes
        let inputs: HashSet<usize> = cfg.variants.iter().map(|e| e.src_index()).collect();

        // enable hardware decoding
        self.decoder.enable_hw_decoder_any();

        // setup decoders
        for input_idx in inputs {
            let stream = info.streams.iter().find(|f| f.index == input_idx).unwrap();
            self.decoder.setup_decoder(stream, None)?;
        }

        // setup scaler/encoders
        for out_stream in &cfg.variants {
            match out_stream {
                VariantStream::Video(v) => {
                    self.encoders.insert(out_stream.id(), v.try_into()?);
                    self.scalers.insert(out_stream.id(), Scaler::new());
                }
                VariantStream::Audio(a) => {
                    let enc = a.try_into()?;
                    let rs = Resample::new(
                        av_get_sample_fmt(cstr!(a.sample_fmt.as_bytes())),
                        a.sample_rate as _,
                        a.channels as _,
                    );
                    self.resampler.insert(out_stream.id(), rs);
                    self.encoders.insert(out_stream.id(), enc);
                }
                _ => continue,
            }
        }

        // Setup copy streams

        // Setup egress
        for e in cfg.egress {
            match e {
                EgressType::HLS(ref c) => {
                    let encoders = self.encoders.iter().filter_map(|(k, v)| {
                        if c.variants.contains(k) {
                            let var = cfg.variants.iter().find(|x| x.id() == *k)?;
                            Some((var, v))
                        } else {
                            None
                        }
                    });

                    let hls = HlsEgress::new(&c.out_dir, 2.0, encoders)?;
                    self.egress.push(Box::new(hls));
                }
                EgressType::Recorder(ref c) => {
                    let encoders = self
                        .encoders
                        .iter()
                        .filter(|(k, v)| c.variants.contains(k))
                        .map(|(_, v)| v);
                    let rec = RecorderEgress::new(c.clone(), encoders)?;
                    self.egress.push(Box::new(rec));
                }
                _ => warn!("{} is not implemented", e),
            }
        }
        Ok(())
    }
}
