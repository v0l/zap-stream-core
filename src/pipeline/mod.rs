use async_trait::async_trait;
use ffmpeg_sys_next::{av_packet_unref, AVPacket};
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

pub mod builder;
pub mod runner;

#[derive(Debug)]
pub enum PipelinePayload {
    /// No output
    Empty,
    /// Skip this step
    Skip,
    /// Raw bytes from ingress
    Bytes(bytes::Bytes),
    /// FFMpeg AVPacket
    AvPacket(*mut AVPacket),
    /// FFMpeg AVFrame
    AvFrame(),
}

unsafe impl Send for PipelinePayload {}
unsafe impl Sync for PipelinePayload {}

impl Drop for PipelinePayload {
    fn drop(&mut self) {
        match self {
            PipelinePayload::AvPacket(pkt) => unsafe {
                av_packet_unref(*pkt);
            },
            _ => {}
        }
    }
}

#[async_trait]
pub trait PipelineStep {
    fn name(&self) -> String;

    async fn process(&mut self, pkg: PipelinePayload) -> Result<PipelinePayload, anyhow::Error>;
}
