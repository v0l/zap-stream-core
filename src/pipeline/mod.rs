use std::fmt::{Display, Formatter};

use anyhow::Error;
use ffmpeg_sys_next::{
    av_frame_clone, av_frame_copy_props, av_frame_free, av_packet_clone, av_packet_copy_props,
    av_packet_free, AVCodecContext, AVFrame, AVPacket, AVStream,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::demux::info::DemuxerInfo;
use crate::egress::EgressConfig;
use crate::variant::VariantStream;

pub mod builder;
pub mod runner;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EgressType {
    /// HLS output egress
    HLS(EgressConfig),

    /// Record streams to local disk
    Recorder(EgressConfig),

    /// Forward streams to another RTMP server
    RTMPForwarder(EgressConfig),
}

impl Display for EgressType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                EgressType::HLS(c) => format!("{}", c),
                EgressType::Recorder(c) => format!("{}", c),
                EgressType::RTMPForwarder(c) => format!("{}", c),
            }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PipelineConfig {
    pub id: Uuid,

    /// Transcoded/Copied stream config
    pub variants: Vec<VariantStream>,

    /// Output muxers
    pub egress: Vec<EgressType>,
}

impl Display for PipelineConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "\nPipeline Config ID={}", self.id)?;
        write!(f, "\nVariants:")?;
        for v in &self.variants {
            write!(f, "\n\t{}", v)?;
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

#[derive(Debug, PartialEq, Clone)]
pub enum AVPacketSource {
    /// AVPacket from demuxer
    Demuxer(*mut AVStream),
    /// AVPacket from an encoder
    Encoder(Uuid),
}

#[derive(Debug, PartialEq, Clone)]
pub enum AVFrameSource {
    /// ACPacket from decoder source stream
    Decoder(*mut AVStream),
    /// AVPacket from frame scaler step
    Scaler(*mut AVStream),
    /// Flush frame (empty)
    Flush,
    /// No context provided, dst_stream manually matched
    None(usize),
}

#[derive(Debug, PartialEq)]
pub enum PipelinePayload {
    /// No output
    Empty,
    /// Raw bytes from ingress
    Bytes(bytes::Bytes),
    /// FFMpeg AVPacket
    AvPacket(*mut AVPacket, AVPacketSource),
    /// FFMpeg AVFrame
    AvFrame(*mut AVFrame, AVFrameSource),
    /// Information about an encoder in this pipeline
    EncoderInfo(Uuid, *const AVCodecContext),
    /// Source stream information provided by the demuxer
    SourceInfo(DemuxerInfo),
    /// Flush pipeline
    Flush,
}

unsafe impl Send for PipelinePayload {}

unsafe impl Sync for PipelinePayload {}

impl Clone for PipelinePayload {
    fn clone(&self) -> Self {
        match self {
            PipelinePayload::AvPacket(p, v) => unsafe {
                assert!(!(**p).data.is_null(), "Cannot clone empty packet");
                let new_pkt = av_packet_clone(*p);
                av_packet_copy_props(new_pkt, *p);
                PipelinePayload::AvPacket(new_pkt, v.clone())
            },
            PipelinePayload::AvFrame(p, v) => unsafe {
                assert!(!(**p).extended_data.is_null(), "Cannot clone empty frame");
                let new_frame = av_frame_clone(*p);
                av_frame_copy_props(new_frame, *p);
                PipelinePayload::AvFrame(new_frame, v.clone())
            },
            PipelinePayload::Empty => PipelinePayload::Empty,
            PipelinePayload::Bytes(b) => PipelinePayload::Bytes(b.clone()),
            PipelinePayload::EncoderInfo(a, b) => PipelinePayload::EncoderInfo(*a, *b),
            PipelinePayload::SourceInfo(a) => PipelinePayload::SourceInfo(a.clone()),
            PipelinePayload::Flush => PipelinePayload::Flush
        }
    }
}

impl Drop for PipelinePayload {
    fn drop(&mut self) {
        match self {
            PipelinePayload::AvPacket(p, _) => unsafe {
                av_packet_free(p);
            },
            PipelinePayload::AvFrame(p, _) => unsafe {
                av_frame_free(p);
            },
            _ => {}
        }
    }
}

pub trait PipelineProcessor {
    fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error>;
}
