use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use uuid::Uuid;

pub mod hls;
pub mod http;
pub mod recorder;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EgressConfig {
    pub name: String,
    pub out_dir: String,
    /// Which variants will be used in this muxer
    pub variants: HashSet<Uuid>,
}

impl Display for EgressConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: out_dir={}", self.name, self.out_dir)?;
        if !self.variants.is_empty() {
            write!(f, "\n\tStreams: ")?;
            for v in &self.variants {
                write!(f, "\n\t\t{}", v)?;
            }
        }
        Ok(())
    }
}

pub trait Egress {
    unsafe fn process_pkt(&mut self, packet: *mut AVPacket, variant: &Uuid)
        -> Result<EgressResult>;
}

#[derive(Debug, Clone)]
pub enum EgressResult {
    /// Nothing to report
    None,
    /// A new segment was created
    NewSegment(NewSegment),
}

/// Basic details of new segment created by a muxer
#[derive(Debug, Clone)]
pub struct NewSegment {
    /// The id of the variant (video or audio)
    pub variant: Uuid,
    /// Segment index
    pub idx: u64,
    /// Duration in seconds
    pub duration: f32,
    /// Path on disk to the segment file
    pub path: PathBuf,
}
