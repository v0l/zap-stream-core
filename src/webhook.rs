use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::{DemuxerInfo, StreamType};
use uuid::Uuid;

use crate::egress::EgressConfig;
use crate::pipeline::{EgressType, PipelineConfig};
use crate::settings::Settings;
use crate::variant::audio::AudioVariant;
use crate::variant::mapping::VariantMapping;
use crate::variant::video::VideoVariant;
use crate::variant::{StreamMapping, VariantStream};

#[derive(Clone)]
pub struct Webhook {
    config: Settings,
}

impl Webhook {
    pub fn new(config: Settings) -> Self {
        Self { config }
    }

    pub fn start(&self, stream_info: &DemuxerInfo) -> PipelineConfig {
        let mut vars: Vec<VariantStream> = vec![];
        if let Some(video_src) = stream_info
            .streams
            .iter()
            .find(|c| c.stream_type == StreamType::Video)
        {
            vars.push(VariantStream::CopyVideo(VariantMapping {
                id: Uuid::new_v4(),
                src_index: video_src.index,
                dst_index: 0,
                group_id: 0,
            }));
            vars.push(VariantStream::Video(VideoVariant {
                mapping: VariantMapping {
                    id: Uuid::new_v4(),
                    src_index: video_src.index,
                    dst_index: 1,
                    group_id: 1,
                },
                width: 1280,
                height: 720,
                fps: video_src.fps,
                bitrate: 3_000_000,
                codec: 27,
                profile: 100,
                level: 51,
                keyframe_interval: video_src.fps as u16 * 2,
                pixel_format: AV_PIX_FMT_YUV420P as u32,
            }));
        }

        if let Some(audio_src) = stream_info
            .streams
            .iter()
            .find(|c| c.stream_type == StreamType::Audio)
        {
            vars.push(VariantStream::CopyAudio(VariantMapping {
                id: Uuid::new_v4(),
                src_index: audio_src.index,
                dst_index: 2,
                group_id: 0,
            }));
            vars.push(VariantStream::Audio(AudioVariant {
                mapping: VariantMapping {
                    id: Uuid::new_v4(),
                    src_index: audio_src.index,
                    dst_index: 3,
                    group_id: 1,
                },
                bitrate: 192_000,
                codec: 86018,
                channels: 2,
                sample_rate: 48_000,
                sample_fmt: "flt".to_owned(),
            }));
        }

        let var_ids = vars.iter().map(|v| v.id()).collect();
        PipelineConfig {
            id: Uuid::new_v4(),
            variants: vars,
            egress: vec![
                EgressType::Recorder(EgressConfig {
                    name: "REC".to_owned(),
                    out_dir: self.config.output_dir.clone(),
                    variants: var_ids,
                }),
                /*EgressType::HLS(EgressConfig {
                    name: "HLS".to_owned(),
                    out_dir: self.config.output_dir.clone(),
                    variants: var_ids,
                }),*/
            ],
        }
    }
}
