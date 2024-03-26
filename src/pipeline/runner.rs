use std::ops::{Add, AddAssign};
use std::time::{Duration, Instant};

use anyhow::Error;
use itertools::Itertools;
use log::info;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::decode::Decoder;
use crate::demux::Demuxer;
use crate::demux::info::{DemuxStreamInfo, StreamChannelType};
use crate::egress::hls::HlsEgress;
use crate::encode::audio::AudioEncoder;
use crate::encode::video::VideoEncoder;
use crate::pipeline::{EgressType, PipelineConfig, PipelinePayload, PipelineProcessor};
use crate::scale::Scaler;
use crate::tag_frame::TagFrame;
use crate::variant::VariantStream;
use crate::webhook::Webhook;

struct PipelineChain {
    pub first: Box<dyn PipelineProcessor + Sync + Send>,
    pub second: Box<dyn PipelineProcessor + Sync + Send>,
}

pub struct PipelineRunner {
    config: PipelineConfig,
    demuxer: Demuxer,
    decoder: Decoder,
    decoder_output: broadcast::Receiver<PipelinePayload>,
    encoders: Vec<PipelineChain>,
    egress: Vec<HlsEgress>,
    started: Instant,
    frame_no: u64,
    stream_info: Option<DemuxStreamInfo>,
    webhook: Webhook,
}

impl PipelineRunner {
    pub fn new(
        config: PipelineConfig,
        webhook: Webhook,
        recv: UnboundedReceiver<bytes::Bytes>,
    ) -> Self {
        let (demux_out, demux_in) = unbounded_channel();
        let (dec_tx, dec_rx) = broadcast::channel::<PipelinePayload>(32);
        Self {
            config,
            demuxer: Demuxer::new(recv, demux_out),
            decoder: Decoder::new(demux_in, dec_tx),
            decoder_output: dec_rx,
            encoders: vec![],
            egress: vec![],
            started: Instant::now(),
            frame_no: 0,
            stream_info: None,
            webhook,
        }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        if let Some(info) = &self.stream_info {
            if let Some(v_stream) = info
                .channels
                .iter()
                .find(|s| s.channel_type == StreamChannelType::Video)
            {
                let duration = self.frame_no as f64 / v_stream.fps as f64;
                let target_time = self.started.add(Duration::from_secs_f64(duration));
                let now = Instant::now();
                if now < target_time {
                    let poll_sleep = target_time - now;
                    std::thread::sleep(poll_sleep);
                }
            }
        }
        if let Some(cfg) = self.demuxer.process()? {
            self.configure_pipeline(cfg)?;
        }
        let frames = self.decoder.process()?;
        if let Some(v) = self.frame_no.checked_add(frames as u64) {
            self.frame_no = v;
        } else {
            panic!("Frame number overflowed, maybe you need a bigger number!");
        }

        // (scalar)-encoder chains
        for sw in &mut self.encoders {
            sw.first.process()?;
            sw.second.process()?;
        }

        // egress outputs
        for eg in &mut self.egress {
            eg.process()?;
        }
        Ok(())
    }

    fn configure_pipeline(&mut self, info: DemuxStreamInfo) -> Result<(), Error> {
        if self.stream_info.is_some() {
            return Err(Error::msg("Pipeline already configured!"));
        }
        self.stream_info = Some(info.clone());

        // re-configure with demuxer info
        self.config = self.webhook.configure(&info);
        info!("Configuring pipeline {}", self.config);

        let video_stream = info
            .channels
            .iter()
            .find(|s| s.channel_type == StreamChannelType::Video);

        if let Some(ref vs) = video_stream {
            for eg in &self.config.egress {
                match eg {
                    EgressType::HLS(cfg) => {
                        let (egress_tx, egress_rx) = unbounded_channel();
                        self.egress
                            .push(HlsEgress::new(egress_rx, self.config.id, cfg.clone()));

                        for v in &cfg.variants {
                            match v {
                                VariantStream::Video(vs) => {
                                    let (sw_tx, sw_rx) = unbounded_channel();
                                    self.encoders.push(PipelineChain {
                                        first: Box::new(Scaler::new(
                                            self.decoder_output.resubscribe(),
                                            sw_tx.clone(),
                                            vs.clone(),
                                        )),
                                        second: Box::new(VideoEncoder::new(
                                            sw_rx,
                                            egress_tx.clone(),
                                            vs.clone(),
                                        )),
                                    });
                                }
                                VariantStream::Audio(va) => {
                                    let (tag_tx, tag_rx) = unbounded_channel();
                                    self.encoders.push(PipelineChain {
                                        first: Box::new(TagFrame::new(
                                            v.clone(),
                                            self.decoder_output.resubscribe(),
                                            tag_tx,
                                        )),
                                        second: Box::new(AudioEncoder::new(
                                            tag_rx,
                                            egress_tx.clone(),
                                            va.clone(),
                                        )),
                                    });
                                }
                                c => {
                                    return Err(Error::msg(format!(
                                        "Variant config not supported {:?}",
                                        c
                                    )));
                                }
                            }
                        }
                    }
                    _ => return Err(Error::msg("Egress config not supported")),
                }
            }
        }

        if self.egress.len() == 0 {
            Err(Error::msg("No egress config, pipeline misconfigured!"))
        } else {
            Ok(())
        }
    }
}
