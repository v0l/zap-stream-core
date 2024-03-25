use std::ops::{Deref, DerefMut};

use ffmpeg_sys_next::{av_frame_clone, av_frame_copy_props, av_frame_free, av_packet_clone, av_packet_copy_props, av_packet_free, AVFrame, AVPacket};
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
    MPEGTS,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HLSEgressConfig {
    pub out_dir: String,
    pub variants: Vec<VariantStream>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
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
    AvPacket(String, *mut AVPacket),
    /// FFMpeg AVFrame
    AvFrame(String, *mut AVFrame),
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
            PipelinePayload::AvPacket(t, p) => unsafe {
                let new_pkt = av_packet_clone(*p);
                av_packet_copy_props(new_pkt, *p);
                PipelinePayload::AvPacket(t.clone(), new_pkt)
            },
            PipelinePayload::AvFrame(t, p) => unsafe {
                let new_frame = av_frame_clone(*p);
                av_frame_copy_props(new_frame, *p);
                PipelinePayload::AvFrame(t.clone(), new_frame)
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
            PipelinePayload::AvPacket(_, p) => unsafe {
                av_packet_free(p);
            },
            PipelinePayload::AvFrame(_, p) => unsafe {
                av_frame_free(p);
            },
            PipelinePayload::SourceInfo(_) => {}
        }
    }
}
