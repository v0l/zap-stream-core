use crate::decode::Decoder;
use crate::demux::info::{DemuxStreamInfo, StreamChannelType};
use crate::demux::Demuxer;
use crate::egress::hls::HlsEgress;
use crate::encode::Encoder;
use crate::pipeline::{EgressType, PipelineConfig, PipelinePayload, PipelineStep};
use crate::scale::Scaler;
use crate::variant::VariantStream;
use anyhow::Error;
use log::info;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

struct ScalerEncoder {
    pub scaler: Scaler,
    pub encoder: Encoder<UnboundedReceiver<PipelinePayload>>,
}

pub struct PipelineRunner {
    config: PipelineConfig,
    demuxer: Demuxer,
    decoder: Decoder,
    decoder_output: broadcast::Receiver<PipelinePayload>,
    scalers: Vec<ScalerEncoder>,
    encoders: Vec<Encoder<broadcast::Receiver<PipelinePayload>>>,
    egress: Vec<HlsEgress>,
}

impl PipelineRunner {
    pub fn new(config: PipelineConfig, recv: UnboundedReceiver<bytes::Bytes>) -> Self {
        let (demux_out, demux_in) = unbounded_channel();
        let (dec_tx, dec_rx) = broadcast::channel::<PipelinePayload>(32);
        Self {
            config,
            demuxer: Demuxer::new(recv, demux_out),
            decoder: Decoder::new(demux_in, dec_tx),
            decoder_output: dec_rx,
            scalers: vec![],
            encoders: vec![],
            egress: vec![],
        }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        if let Some(cfg) = self.demuxer.process()? {
            self.configure_pipeline(cfg)?;
        }
        self.decoder.process()?;
        for sw in &mut self.scalers {
            sw.scaler.process()?;
            sw.encoder.process()?;
            for eg in &mut self.egress {
                eg.process()?;
            }
        }
        Ok(())
    }

    fn configure_pipeline(&mut self, info: DemuxStreamInfo) -> Result<(), Error> {
        // configure scalers
        if self.scalers.len() != 0 {
            return Err(Error::msg("Pipeline already configured!"));
        }
        info!("Configuring pipeline {:?}", info);

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
                            let (var_tx, var_rx) = unbounded_channel();
                            match v {
                                VariantStream::Video(vs) => {
                                    self.scalers.push(ScalerEncoder {
                                        scaler: Scaler::new(
                                            self.decoder_output.resubscribe(),
                                            var_tx.clone(),
                                            vs.clone(),
                                        ),
                                        encoder: Encoder::new(var_rx, egress_tx.clone(), v.clone()),
                                    });
                                }
                                VariantStream::Audio(_) => {
                                    self.encoders.push(Encoder::new(
                                        self.decoder_output.resubscribe(),
                                        egress_tx.clone(),
                                        v.clone(),
                                    ));
                                }
                                c => {
                                    return Err(Error::msg(format!(
                                        "Variant config not supported {:?}",
                                        c
                                    )))
                                }
                            }
                        }
                    }
                    _ => return Err(Error::msg("Egress config not supported")),
                }
            }
        }
        Ok(())
    }
}
