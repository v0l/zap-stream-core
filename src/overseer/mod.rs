use crate::ingress::ConnectionInfo;

#[cfg(feature = "local-overseer")]
use crate::overseer::local::LocalOverseer;
#[cfg(feature = "webhook-overseer")]
use crate::overseer::webhook::WebhookOverseer;
#[cfg(feature = "zap-stream")]
use crate::overseer::zap_stream::ZapStreamOverseer;
use crate::pipeline::PipelineConfig;
#[cfg(any(
    feature = "local-overseer",
    feature = "webhook-overseer",
    feature = "zap-stream"
))]
use crate::settings::OverseerConfig;
use crate::settings::Settings;
use crate::variant::audio::AudioVariant;
use crate::variant::mapping::VariantMapping;
use crate::variant::video::VideoVariant;
use crate::variant::VariantStream;
use anyhow::Result;
use async_trait::async_trait;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use std::cmp::PartialEq;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(feature = "local-overseer")]
mod local;

#[cfg(feature = "webhook-overseer")]
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
    /// Check all streams
    async fn check_streams(&self) -> Result<()>;

    /// Set up a new streaming pipeline
    async fn start_stream(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig>;

    /// A new segment (HLS etc.) was generated for a stream variant
    ///
    /// This handler is usually used for distribution / billing
    async fn on_segment(
        &self,
        pipeline_id: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()>;

    /// At a regular interval, pipeline will emit one of the frames for processing as a
    /// thumbnail
    async fn on_thumbnail(
        &self,
        pipeline_id: &Uuid,
        width: usize,
        height: usize,
        path: &PathBuf,
    ) -> Result<()>;

    /// Stream is finished
    async fn on_end(&self, pipeline_id: &Uuid) -> Result<()>;
}

impl Settings {
    pub async fn get_overseer(&self) -> Result<Arc<dyn Overseer>> {
        match &self.overseer {
            #[cfg(feature = "local-overseer")]
            OverseerConfig::Local => Ok(Arc::new(LocalOverseer::new())),
            #[cfg(feature = "webhook-overseer")]
            OverseerConfig::Webhook { url } => Ok(Arc::new(WebhookOverseer::new(&url))),
            #[cfg(feature = "zap-stream")]
            OverseerConfig::ZapStream {
                nsec: private_key,
                database,
                lnd,
                relays,
                blossom,
                cost,
            } => Ok(Arc::new(
                ZapStreamOverseer::new(
                    &self.output_dir,
                    &self.public_url,
                    private_key,
                    database,
                    lnd,
                    relays,
                    blossom,
                    *cost,
                )
                .await?,
            )),
            _ => {
                panic!("Unsupported overseer");
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
            codec: "libx264".to_string(),
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
            codec: "aac".to_string(),
            channels: 2,
            sample_rate: 48_000,
            sample_fmt: "fltp".to_owned(),
        }));
    }

    Ok(vars)
}
