use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[cfg(feature = "egress-hls")]
mod hls;
#[cfg(feature = "egress-hls")]
pub use hls::*;

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Hash, Eq)]
pub enum SegmentType {
    MPEGTS,
    FMP4,
}

impl Display for SegmentType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SegmentType::MPEGTS => write!(f, "MPEGTS"),
            SegmentType::FMP4 => write!(f, "fMP4"),
        }
    }
}
