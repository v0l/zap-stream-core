use std::fmt::{Display, Formatter};

use crate::egress::EgressConfig;
use crate::variant::VariantStream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

impl EgressType {
    pub fn config(&self) -> &EgressConfig {
        match self {
            EgressType::HLS(c) => c,
            EgressType::Recorder(c) => c,
            EgressType::RTMPForwarder(c) => c,
        }
    }
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
