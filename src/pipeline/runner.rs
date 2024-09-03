use std::ops::Sub;
use std::time::Instant;

use anyhow::Error;
use log::info;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::decode::Decoder;
use crate::demux::info::DemuxerInfo;
use crate::demux::Demuxer;
use crate::egress::hls::HlsEgress;
use crate::egress::recorder::RecorderEgress;
use crate::encode::audio::AudioEncoder;
use crate::encode::video::VideoEncoder;
use crate::ingress::ConnectionInfo;
use crate::pipeline::{
    AVPacketSource, EgressType, PipelineConfig, PipelinePayload, PipelineProcessor,
};
use crate::scale::Scaler;
use crate::variant::{StreamMapping, VariantStream};
use crate::webhook::Webhook;

type BoxedProcessor = Box<dyn PipelineProcessor + Sync + Send>;

/// Resample/Encode
struct Transcoder {
    pub variant: Uuid,

    /// A resampler which can take decoded sames (Audio or Video)
    pub sampler: Option<BoxedProcessor>,

    /// The encoder which will encode the resampled frames
    pub encoder: BoxedProcessor,
}

///
/// |----------------------------------------------------|
/// | Demuxer
pub struct PipelineRunner {
    config: PipelineConfig,
    info: ConnectionInfo,

    demuxer: Demuxer,
    decoder: Decoder,
    transcoders: Vec<Transcoder>,
    muxers: Vec<BoxedProcessor>,

    started: Instant,
    frame_no: u64,
    stream_info: Option<DemuxerInfo>,
    webhook: Webhook,
}

impl PipelineRunner {
    pub fn new(
        config: PipelineConfig,
        info: ConnectionInfo,
        webhook: Webhook,
        recv: UnboundedReceiver<bytes::Bytes>,
    ) -> Self {
        Self {
            config,
            info,
            demuxer: Demuxer::new(recv),
            decoder: Decoder::new(),
            transcoders: vec![],
            muxers: vec![],
            started: Instant::now(),
            frame_no: 0,
            stream_info: None,
            webhook,
        }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        if self.stream_info.is_none() {
            if let Some(cfg) = self.demuxer.try_probe()? {
                self.configure_pipeline(&cfg)?;
                for mux in &mut self.muxers {
                    mux.process(PipelinePayload::SourceInfo(cfg.clone()))?;
                }
                self.stream_info = Some(cfg);
            } else {
                return Ok(());
            }
        }

        let demux_pkg = unsafe { self.demuxer.get_packet() }?;

        let src_index = if let PipelinePayload::AvPacket(_, s) = &demux_pkg {
            if let AVPacketSource::Demuxer(s) = s {
                unsafe { (*(*s)).index }
            } else {
                -1
            }
        } else {
            -1
        };
        let pkg_variant = self.config.variants.iter().find(|v| match v {
            VariantStream::Video(vx) => vx.src_index() as i32 == src_index,
            VariantStream::Audio(ax) => ax.src_index() as i32 == src_index,
            _ => false,
        });
        let transcoded_pkgs = if let Some(var) = pkg_variant {
            let frames = self.decoder.process(demux_pkg.clone())?;
            if let VariantStream::Video(_) = var {
                self.frame_no += frames.len() as u64;
                //TODO: Account for multiple video streams in
            }

            let mut pkgs = Vec::new();
            for frame in &frames {
                for tran in &mut self.transcoders {
                    let frames = if let Some(ref mut smp) = tran.sampler {
                        smp.process(frame.clone())?
                    } else {
                        vec![frame.clone()]
                    };

                    for frame in frames {
                        for pkg in tran.encoder.process(frame)? {
                            pkgs.push(pkg);
                        }
                    }
                }
            }
            pkgs
        } else {
            vec![]
        };

        // mux
        for pkg in transcoded_pkgs {
            for ref mut mux in &mut self.muxers {
                mux.process(pkg.clone())?;
            }
        }
        for ref mut mux in &mut self.muxers {
            mux.process(demux_pkg.clone())?;
        }

        let elapsed = Instant::now().sub(self.started).as_secs_f32();
        if elapsed >= 2f32 {
            info!("Average fps: {:.2}", self.frame_no as f32 / elapsed);
            self.started = Instant::now();
            self.frame_no = 0;
        }
        Ok(())
    }

    /// Setup pipeline based on the demuxer info
    fn configure_pipeline(&mut self, info: &DemuxerInfo) -> Result<(), Error> {
        // re-configure with demuxer info
        self.config = self.webhook.start(info);
        info!("Configuring pipeline {}", self.config);
        if self.config.egress.iter().any(|x| match x {
            EgressType::HLS(_) => true,
            _ => false,
        }) {
            info!(
                "Livestream url: http://localhost:8080/{}/live.m3u8",
                self.config.id
            );
        }

        // configure transcoders
        for var in &self.config.variants {
            match var {
                VariantStream::Video(v) => {
                    let scaler = Scaler::new(v.clone());
                    let encoder = VideoEncoder::new(v.clone());
                    self.transcoders.push(Transcoder {
                        variant: v.id(),
                        sampler: Some(Box::new(scaler)),
                        encoder: Box::new(encoder),
                    });
                }
                VariantStream::Audio(a) => {
                    let encoder = AudioEncoder::new(a.clone());
                    self.transcoders.push(Transcoder {
                        variant: a.id(),
                        sampler: None,
                        encoder: Box::new(encoder),
                    });
                }
                _ => {
                    //ignored
                }
            }
        }

        // configure muxers
        for mux in &self.config.egress {
            match mux {
                EgressType::HLS(c) => {
                    let mut hls =
                        HlsEgress::new(Uuid::new_v4(), c.clone(), self.config.variants.clone());
                    hls.setup_muxer()?;
                    self.muxers.push(Box::new(hls));
                }
                EgressType::Recorder(c) => {
                    let recorder = RecorderEgress::new(
                        Uuid::new_v4(),
                        c.clone(),
                        self.config.variants.clone(),
                    );
                    self.muxers.push(Box::new(recorder));
                }
                EgressType::RTMPForwarder(c) => {
                    todo!("Implement this")
                }
            }
        }

        if self.muxers.is_empty() {
            Err(Error::msg("No egress config, pipeline misconfigured!"))
        } else {
            Ok(())
        }
    }
}
