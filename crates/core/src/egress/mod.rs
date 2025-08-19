use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AVPacket, AVStream};
use ffmpeg_rs_raw::Encoder;
use std::path::PathBuf;
use uuid::Uuid;

pub mod hls;
pub mod recorder;
#[cfg(feature = "egress-rtmp")]
pub mod rtmp;

pub trait Egress {
    unsafe fn process_pkt(&mut self, packet: *mut AVPacket, variant: &Uuid)
        -> Result<EgressResult>;
    unsafe fn reset(&mut self) -> Result<EgressResult>;
}

#[derive(Debug, Clone)]
pub enum EgressResult {
    /// Nothing to report
    None,
    /// Egress created/deleted some segments
    Segments {
        created: Vec<EgressSegment>,
        deleted: Vec<EgressSegment>,
    },
}

/// Basic details of new segment created by a muxer
#[derive(Debug, Clone)]
pub struct EgressSegment {
    /// The id of the variant (video or audio)
    pub variant: Uuid,
    /// Segment index
    pub idx: u64,
    /// Duration in seconds
    pub duration: f32,
    /// Path on disk to the segment file
    pub path: PathBuf,
    /// SHA-256 hash of the file
    pub sha256: [u8; 32],
}

pub enum EncoderOrSourceStream<'a> {
    Encoder(&'a Encoder),
    SourceStream(*mut AVStream),
}
