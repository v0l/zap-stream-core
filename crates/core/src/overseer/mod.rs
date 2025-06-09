use crate::ingress::ConnectionInfo;

use crate::egress::EgressSegment;
use crate::pipeline::PipelineConfig;
use anyhow::Result;
use async_trait::async_trait;
use std::cmp::PartialEq;
use std::path::PathBuf;
use uuid::Uuid;

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
    pub channels: u8,
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

    /// A new segment(s) (HLS etc.) was generated for a stream variant
    ///
    /// This handler is usually used for distribution / billing
    async fn on_segments(
        &self,
        pipeline_id: &Uuid,
        added: &Vec<EgressSegment>,
        deleted: &Vec<EgressSegment>,
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
