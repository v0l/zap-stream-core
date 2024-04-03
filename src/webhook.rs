use uuid::Uuid;

use crate::demux::info::{DemuxStreamInfo, StreamChannelType};
use crate::egress::EgressConfig;
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

    pub async fn start(&self, _connection_info: ConnectionInfo) -> Result<(), anyhow::Error> {
        Ok(())
    }

    pub fn configure(&self, stream_info: &DemuxStreamInfo) -> PipelineConfig {
        let mut vars: Vec<VariantStream> = vec![];
        if let Some(video_src) = stream_info
            .channels
            .iter()
            .find(|c| c.channel_type == StreamChannelType::Video)
        {
            vars.push(VariantStream::Video(VideoVariant {
                id: Uuid::new_v4(),
                src_index: video_src.index,
                dst_index: 0,
                width: 1280,
                height: 720,
                fps: video_src.fps as u16,
                bitrate: 3_000_000,
                codec: 27,
                profile: 100,
                level: 51,
                keyframe_interval: 2,
            }));
            vars.push(VariantStream::Video(VideoVariant {
                id: Uuid::new_v4(),
                src_index: video_src.index,
                dst_index: 1,
                width: 640,
                height: 360,
                fps: video_src.fps as u16,
                bitrate: 1_000_000,
                codec: 27,
                profile: 100,
                level: 51,
                keyframe_interval: 2,
            }));
        }

        if let Some(audio_src) = stream_info
            .channels
            .iter()
            .find(|c| c.channel_type == StreamChannelType::Audio)
        {
            vars.push(VariantStream::Audio(AudioVariant {
                id: Uuid::new_v4(),
                src_index: audio_src.index,
                dst_index: 0,
                bitrate: 320_000,
                codec: 86018,
                channels: 2,
                sample_rate: 48_000,
                sample_fmt: "s16".to_owned(),
            }));
            vars.push(VariantStream::Audio(AudioVariant {
                id: Uuid::new_v4(),
                src_index: audio_src.index,
                dst_index: 1,
                bitrate: 220_000,
                codec: 86018,
                channels: 2,
                sample_rate: 48_000,
                sample_fmt: "s16".to_owned(),
            }));
        }

        PipelineConfig {
            id: Uuid::new_v4(),
            recording: vec![],
            egress: vec![
                EgressType::Recorder(EgressConfig {
                    name: "Recorder".to_owned(),
                    out_dir: self.config.output_dir.clone(),
                    variants: vars.clone(),
                }),
                /*EgressType::MPEGTS(EgressConfig {
                    name: "MPEGTS".to_owned(),
                    out_dir: self.config.output_dir.clone(),
                    variants: vars.clone(),
                }),*/
            ],
        }
    }
}
