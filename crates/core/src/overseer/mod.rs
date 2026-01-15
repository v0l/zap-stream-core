use crate::egress::EgressSegment;
use crate::ingress::{ConnectionInfo, IngressInfo};
use crate::metrics::EndpointStats;
use crate::pipeline::PipelineConfig;
use crate::pipeline::PipelineStats;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug)]
pub enum StatsType {
    Ingress(EndpointStats),
    Pipeline(PipelineStats),
    Egress(EndpointStats),
}

#[derive(Debug, Clone)]
pub enum ConnectResult {
    Allow {
        /// Enable dumping stream data to disk for debugging purposes
        enable_stream_dump: bool,
        /// Replace the stream/pipeline id
        stream_id_override: Option<Uuid>,
    },
    Deny {
        reason: String,
    },
}

#[async_trait]
/// The control process that oversees streaming operations
pub trait Overseer: Send + Sync {
    /// Check all streams
    async fn check_streams(&self) -> Result<()>;

    /// Authorize connection for user
    async fn connect(&self, connection_info: &ConnectionInfo) -> Result<ConnectResult>;

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

    /// Force update stream
    async fn on_update(&self, pipeline_id: &Uuid) -> Result<()>;

    /// Stats emitted by the pipeline periodically
    async fn on_stats(&self, pipeline_id: &Uuid, stats: StatsType) -> Result<()>;
}
