use std::ops::{Deref, DerefMut};

use async_trait::async_trait;
use ffmpeg_sys_next::{AVFrame, AVPacket};
use serde::{Deserialize, Serialize};

use crate::demux::info::DemuxStreamInfo;
use crate::variant::VariantStream;

pub mod builder;
pub mod runner;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EgressType {
    HLS(HLSEgressConfig),
    DASH,
    WHEP,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HLSEgressConfig {
    pub variants: Vec<VariantStream>,

    /// FFMPEG stream mapping string
    ///
    /// v:0,a:0 v:1,a:0, v:2,a:1 etc..
    pub stream_map: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub id: uuid::Uuid,
    pub recording: Vec<VariantStream>,
    pub egress: Vec<EgressType>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PipelinePayload {
    /// No output
    Empty,
    /// Raw bytes from ingress
    Bytes(bytes::Bytes),
    /// FFMpeg AVPacket
    AvPacket(*mut AVPacket),
    /// FFMpeg AVFrame
    AvFrame(*mut AVFrame),
    /// Information about the input stream
    SourceInfo(DemuxStreamInfo),
}

unsafe impl Send for PipelinePayload {}
unsafe impl Sync for PipelinePayload {}

#[async_trait]
pub trait PipelineStep {
    fn name(&self) -> String;
    async fn process(&mut self, pkg: &PipelinePayload) -> Result<PipelinePayload, anyhow::Error>;
}
