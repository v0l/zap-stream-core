use std::fmt::Display;

use ffmpeg_sys_next::{AV_LEVEL_UNKNOWN, AV_PROFILE_H264_HIGH};
use ffmpeg_sys_next::AVCodecID::{AV_CODEC_ID_AAC, AV_CODEC_ID_H264};
use uuid::Uuid;

use crate::ingress::ConnectionInfo;
use crate::pipeline::{EgressType, HLSEgressConfig, PipelineConfig};
use crate::variant::{AudioVariant, VariantStream, VideoVariant};

#[derive(Clone)]
pub struct Webhook {
    url: String,
}

impl Webhook {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub async fn start(
        &self,
        connection_info: ConnectionInfo,
    ) -> Result<PipelineConfig, anyhow::Error> {
        let video_var = VideoVariant {
            id: Uuid::new_v4(),
            src_index: 0,
            dst_index: 0,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate: 3_000_000,
            codec: 27,
            profile: 100,
            level: 1,
            keyframe_interval: 2,
        };
        let video_var_2 = VideoVariant {
            id: Uuid::new_v4(),
            src_index: 0,
            dst_index: 0,
            width: 640,
            height: 360,
            fps: 30,
            bitrate: 1_000_000,
            codec: 27,
            profile: 100,
            level: 1,
            keyframe_interval: 2,
        };
        let audio_var = AudioVariant {
            id: Uuid::new_v4(),
            src_index: 1,
            dst_index: 0,
            bitrate: 320_000,
            codec: 86018,
            channels: 2,
            sample_rate: 44_100,
            sample_fmt: "fltp".to_owned(),
        };

        let audio_var_2 = AudioVariant {
            id: Uuid::new_v4(),
            src_index: 1,
            dst_index: 0,
            bitrate: 220_000,
            codec: 86018,
            channels: 2,
            sample_rate: 44_100,
            sample_fmt: "fltp".to_owned(),
        };

        Ok(PipelineConfig {
            id: Uuid::new_v4(),
            egress: vec![EgressType::HLS(HLSEgressConfig {
                variants: vec![
                    VariantStream::Video(video_var),
                    VariantStream::Video(video_var_2),
                    VariantStream::Audio(audio_var),
                    VariantStream::Audio(audio_var_2),
                ],
                stream_map: "v:0,a:0 v:1,a:1".to_owned(),
            })],
            recording: vec![VariantStream::CopyVideo(0), VariantStream::CopyAudio(1)],
        })
    }
}
