use crate::egress::EgressConfig;
use crate::ingress::ConnectionInfo;
use crate::overseer::webhook::WebhookOverseer;
#[cfg(feature = "zap-stream")]
use crate::overseer::zap_stream::ZapStreamOverseer;
use crate::pipeline::{EgressType, PipelineConfig};
use crate::settings::{OverseerConfig, Settings};
use crate::variant::audio::AudioVariant;
use crate::variant::mapping::VariantMapping;
use crate::variant::video::VideoVariant;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use std::cmp::PartialEq;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

mod webhook;
#[cfg(feature = "zap-stream")]
mod zap_stream;

/// A copy of [ffmpeg_rs_raw::DemuxerInfo] without internal ptr
#[derive(PartialEq, Clone)]
pub struct IngressInfo {
    pub bitrate: usize,
    pub streams: Vec<IngressStream>,
}

/// A copy of [ffmpeg_rs_raw::StreamInfo] without ptr
#[derive(PartialEq, Clone)]
pub struct IngressStream {
    pub index: usize,
    pub stream_type: IngressStreamType,
    pub codec: isize,
    pub format: isize,
    pub width: usize,
    pub height: usize,
    pub fps: f32,
    pub sample_rate: usize,
    pub language: String,
}

#[derive(PartialEq, Eq, Clone)]
pub enum IngressStreamType {
    Video,
    Audio,
    Subtitle,
}

#[async_trait]
/// The control process that oversees streaming operations
pub trait Overseer: Send + Sync {
    /// Set up a new streaming pipeline
    async fn configure_pipeline(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig>;

    /// A new segment (HLS etc.) was generated for a stream variant
    ///
    /// This handler is usually used for distribution / billing
    async fn new_segment(
        &self,
        pipeline: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()>;
}

impl Settings {
    pub async fn get_overseer(&self) -> Result<Arc<dyn Overseer>> {
        match &self.overseer {
            OverseerConfig::Static { egress_types } => Ok(Arc::new(StaticOverseer::new(
                &self.output_dir,
                egress_types,
            ))),
            OverseerConfig::Webhook { url } => Ok(Arc::new(WebhookOverseer::new(&url))),
            OverseerConfig::ZapStream {
                nsec: private_key,
                database,
                lnd,
                relays,
            } => {
                #[cfg(not(feature = "zap-stream"))]
                panic!("zap.stream overseer is not enabled");

                #[cfg(feature = "zap-stream")]
                Ok(Arc::new(
                    ZapStreamOverseer::new(private_key, database, lnd, relays).await?,
                ))
            }
        }
    }
}

pub(crate) fn get_default_variants(info: &IngressInfo) -> Result<Vec<VariantStream>> {
    let mut vars: Vec<VariantStream> = vec![];
    if let Some(video_src) = info
        .streams
        .iter()
        .find(|c| c.stream_type == IngressStreamType::Video)
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

    if let Some(audio_src) = info
        .streams
        .iter()
        .find(|c| c.stream_type == IngressStreamType::Audio)
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
            sample_fmt: "fltp".to_owned(),
        }));
    }

    Ok(vars)
}
/// Simple static file output without any access controls
struct StaticOverseer {}

impl StaticOverseer {
    fn new(out_dir: &str, egress_types: &Vec<String>) -> Self {
        Self {}
    }
}

#[async_trait]
impl Overseer for StaticOverseer {
    async fn configure_pipeline(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig> {
        let vars = get_default_variants(stream_info)?;
        let var_ids = vars.iter().map(|v| v.id()).collect();
        Ok(PipelineConfig {
            id: Utc::now().timestamp() as u64,
            variants: vars,
            egress: vec![
                /*EgressType::Recorder(EgressConfig {
                    name: "REC".to_owned(),
                    out_dir: self.config.output_dir.clone(),
                    variants: var_ids,
                }),*/
                EgressType::HLS(EgressConfig {
                    name: "HLS".to_owned(),
                    // TODO: this is temp, webhook should not need full config
                    out_dir: "out".to_string(),
                    variants: var_ids,
                }),
            ],
        })
    }

    async fn new_segment(
        &self,
        pipeline: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()> {
        todo!()
    }
}
