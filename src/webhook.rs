use std::fmt::Display;

use uuid::Uuid;

use crate::demux::info::{DemuxStreamInfo, StreamChannelType};
use crate::egress::hls::HLSEgressConfig;
use crate::ingress::ConnectionInfo;
use crate::pipeline::{EgressType, PipelineConfig};
use crate::settings::Settings;
use crate::variant::{AudioVariant, VariantStream, VideoVariant};

#[derive(Clone)]
pub struct Webhook {
    config: Settings,
}

impl Webhook {
    pub fn new(config: Settings) -> Self {
        Self { config }
    }

    pub async fn start(&self, connection_info: ConnectionInfo) -> Result<(), anyhow::Error> {
        Ok(())
    }

    pub fn configure(&self, stream_info: &DemuxStreamInfo) -> PipelineConfig {
        let mut vars: Vec<VariantStream> = vec![];
        vars.push(VariantStream::Video(VideoVariant {
            id: Uuid::new_v4(),
            src_index: 0,
            dst_index: 0,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate: 3_000_000,
            codec: 27,
            profile: 100,
            level: 51,
            keyframe_interval: 2,
        }));
        vars.push(VariantStream::Video(VideoVariant {
            id: Uuid::new_v4(),
            src_index: 0,
            dst_index: 1,
            width: 640,
            height: 360,
            fps: 30,
            bitrate: 1_000_000,
            codec: 27,
            profile: 100,
            level: 51,
            keyframe_interval: 2,
        }));
        let has_audio = stream_info
            .channels
            .iter()
            .any(|c| c.channel_type == StreamChannelType::Audio);
        if has_audio {
            vars.push(VariantStream::Audio(AudioVariant {
                id: Uuid::new_v4(),
                src_index: 1,
                dst_index: 0,
                bitrate: 320_000,
                codec: 86018,
                channels: 2,
                sample_rate: 44_100,
                sample_fmt: "fltp".to_owned(),
            }));
            vars.push(VariantStream::Audio(AudioVariant {
                id: Uuid::new_v4(),
                src_index: 1,
                dst_index: 1,
                bitrate: 220_000,
                codec: 86018,
                channels: 2,
                sample_rate: 44_100,
                sample_fmt: "fltp".to_owned(),
            }));
        }

        PipelineConfig {
            id: Uuid::new_v4(),
            recording: vec![],
            egress: vec![EgressType::HLS(HLSEgressConfig {
                out_dir: self.config.output_dir.clone(),
                variants: vars,
            })],
        }
    }
}
