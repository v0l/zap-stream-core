use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
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
    unsafe fn process_pkt(&mut self, packet: *mut AVPacket, variant: &Uuid) -> Result<()>;
}
