use std::ops::{Deref, DerefMut};

use async_trait::async_trait;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_free, av_frame_ref, av_packet_alloc, av_packet_free, av_packet_ref,
    AVFrame, AVPacket,
};
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

#[derive(Debug, PartialEq)]
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

impl Clone for PipelinePayload {
    fn clone(&self) -> Self {
        match self {
            PipelinePayload::Empty => PipelinePayload::Empty,
            PipelinePayload::Bytes(b) => PipelinePayload::Bytes(b.clone()),
            PipelinePayload::AvPacket(p) => unsafe {
                let new_pkt = av_packet_alloc();
                av_packet_ref(new_pkt, *p);
                PipelinePayload::AvPacket(new_pkt)
            },
            PipelinePayload::AvFrame(p) => unsafe {
                let new_frame = av_frame_alloc();
                av_frame_ref(new_frame, *p);
                PipelinePayload::AvFrame(new_frame)
            },
            PipelinePayload::SourceInfo(i) => PipelinePayload::SourceInfo(i.clone()),
        }
    }
}

impl Drop for PipelinePayload {
    fn drop(&mut self) {
        match self {
            PipelinePayload::Empty => {}
            PipelinePayload::Bytes(_) => {}
            PipelinePayload::AvPacket(p) => unsafe {
                av_packet_free(p);
            },
            PipelinePayload::AvFrame(p) => unsafe {
                av_frame_free(p);
            },
            PipelinePayload::SourceInfo(_) => {}
        }
    }
}

#[async_trait]
pub trait PipelineStep {
    fn name(&self) -> String;
    async fn process(&mut self, pkg: &PipelinePayload) -> Result<PipelinePayload, anyhow::Error>;
}
