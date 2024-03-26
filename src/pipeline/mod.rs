use std::fmt::{Display, Formatter};

use anyhow::Error;
use ffmpeg_sys_next::{av_frame_clone, av_frame_copy_props, av_frame_free, av_packet_clone, av_packet_copy_props, av_packet_free, AVFrame, AVPacket};
use serde::{Deserialize, Serialize};

use crate::demux::info::DemuxStreamInfo;
use crate::egress::hls::HLSEgressConfig;
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

impl Display for EgressType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                EgressType::HLS(c) => format!("{}", c),
                EgressType::DASH => "DASH".to_owned(),
                EgressType::WHEP => "WHEP".to_owned(),
                EgressType::MPEGTS => "MPEGTS".to_owned(),
            }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PipelineConfig {
    pub id: uuid::Uuid,
    pub recording: Vec<VariantStream>,
    pub egress: Vec<EgressType>,
}

impl Display for PipelineConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "\nPipeline Config ID={}", self.id)?;
        if !self.recording.is_empty() {
            write!(f, "\nRecording:")?;
            for r in &self.recording {
                write!(f, "\n\t{}", r)?;
            }
        }
        if !self.egress.is_empty() {
            write!(f, "\nEgress:")?;
            for e in &self.egress {
                write!(f, "\n\t{}", e)?;
            }
        }
        Ok(())
    }
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
    AvFrame(String, *mut AVFrame, usize),
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
            PipelinePayload::AvFrame(t, p, idx) => unsafe {
                let new_frame = av_frame_clone(*p);
                av_frame_copy_props(new_frame, *p);
                PipelinePayload::AvFrame(t.clone(), new_frame, *idx)
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
            PipelinePayload::AvFrame(_, p, _) => unsafe {
                av_frame_free(p);
            },
            PipelinePayload::SourceInfo(_) => {}
        }
    }
}

pub trait PipelineProcessor {
    fn process(&mut self) -> Result<(), Error>;
}