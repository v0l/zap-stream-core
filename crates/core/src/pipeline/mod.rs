use std::collections::HashSet;
use std::fmt::{Display, Formatter};

use crate::overseer::IngressInfo;
use crate::variant::VariantStream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod runner;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EgressType {
    /// HLS output egress
    HLS(HashSet<Uuid>, f32),

    /// Record streams to local disk
    Recorder(HashSet<Uuid>),

    /// Forward streams to another RTMP server
    RTMPForwarder(HashSet<Uuid>, String),
}

impl EgressType {
    pub fn variants(&self) -> &HashSet<Uuid> {
        match self {
            EgressType::HLS(a, _) => a,
            EgressType::Recorder(a) => a,
            EgressType::RTMPForwarder(a, _) => a,
        }
    }
}

impl Display for EgressType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EgressType::HLS(_, _) => write!(f, "HLS"),
            EgressType::Recorder(_) => write!(f, "Recorder"),
            EgressType::RTMPForwarder(_, d) => write!(f, "RTMPForwarder => {}", d),
        }
    }
}

#[derive(Clone)]
pub struct PipelineConfig {
    /// Transcoded/Copied stream config
    pub variants: Vec<VariantStream>,
    /// Output muxers
    pub egress: Vec<EgressType>,
    /// Source stream information for placeholder generation
    pub ingress_info: IngressInfo,
    /// Primary source video stream
    pub video_src: usize,
    /// Primary audio source stream
    pub audio_src: Option<usize>,
}

impl Display for PipelineConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "\nPipeline Config:")?;
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
