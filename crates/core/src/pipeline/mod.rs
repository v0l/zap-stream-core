use std::fmt::{Display, Formatter};

use crate::egress::{EgressEncoderConfig, EncoderParam, EncoderParams};
use crate::mux::SegmentType;
use crate::overseer::{IngressInfo, IngressStream, IngressStreamType};
use crate::variant::{VariantGroup, VariantStream};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod runner;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum EgressType {
    /// HLS output egress
    HLS {
        /// Unique id of this egress
        id: Uuid,
        /// Segment length in seconds
        segment_length: f32,
        /// Segment type
        segment_type: SegmentType,
    },
    /// Record streams to local disk
    Recorder {
        /// Unique id of this egress
        id: Uuid,
        /// Desired video size height in pixels
        height: u16,
    },
    /// Forward streams to another RTMP server
    RTMPForwarder {
        /// Unique id of this egress
        id: Uuid,
        /// Destination RTMP url
        destination: String,
    },
    /// Media over Quic egress
    Moq {
        /// Unique id of this egress
        id: Uuid,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EgressConfig {
    pub kind: EgressType,
    /// Groups of variants
    pub variants: Vec<VariantGroup>,
}

impl EgressType {
    pub fn id(&self) -> Uuid {
        match self {
            EgressType::HLS { id, .. } => *id,
            EgressType::Recorder { id, .. } => *id,
            EgressType::RTMPForwarder { id, .. } => *id,
            EgressType::Moq { id } => *id,
        }
    }

    /// Get the encoder params this egress needs to process encoded packets
    pub fn get_encoder_params(
        &self,
        stream: &IngressStream,
        input_params: &EncoderParams,
    ) -> Option<EgressEncoderConfig> {
        let mut p = EgressEncoderConfig::default_h264(stream)?;
        p.codec_params.extend(input_params.clone());
        if matches!(self, EgressType::HLS { .. }) && stream.stream_type == IngressStreamType::Audio
        {
            // for HLS force the audio bitrate to always be 192khz
            p.codec_params
                .add_param(EncoderParam::Bitrate { value: 192_000 });
        }
        Some(p)
    }
}

#[derive(Clone)]
pub struct PipelineConfig {
    /// Transcoded/Copied stream config
    pub variants: Vec<VariantStream>,
    /// Output muxers
    pub egress: Vec<EgressConfig>,
    /// Source stream information for placeholder generation
    pub ingress_info: IngressInfo,
    /// Primary source video stream
    pub video_src: usize,
    /// Primary audio source stream
    pub audio_src: Option<usize>,
}

impl Display for PipelineConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "PipelineConfig:")?;
        writeln!(
            f,
            "├── Sources: video={}, audio={:?}",
            self.video_src, self.audio_src
        )?;

        // Display ingress streams
        writeln!(
            f,
            "├── Ingress Streams ({}):",
            self.ingress_info.streams.len()
        )?;
        for (i, stream) in self.ingress_info.streams.iter().enumerate() {
            let is_last = i == self.ingress_info.streams.len() - 1;
            let prefix = if is_last {
                "│   └──"
            } else {
                "│   ├──"
            };
            writeln!(f, "{} {}", prefix, stream)?;
        }

        writeln!(f, "├── Variants ({}):", self.variants.len())?;
        for (i, variant) in self.variants.iter().enumerate() {
            let prefix = if i == self.variants.len() - 1 {
                "│   └──"
            } else {
                "│   ├──"
            };
            writeln!(f, "{} {}", prefix, variant)?;
        }

        writeln!(f, "└── Egress ({}):", self.egress.len())?;
        let egress_count = self.egress.len();
        for (i, egress) in self.egress.iter().enumerate() {
            let is_last_egress = i == egress_count - 1;
            let egress_prefix = if is_last_egress {
                "    └──"
            } else {
                "    ├──"
            };
            let child_prefix = if is_last_egress {
                "       "
            } else {
                "    │  "
            };

            // Get egress type name
            let egress_name = match &egress.kind {
                EgressType::HLS { id, .. } => format!("HLS ({})", id),
                EgressType::Recorder { id, height } => format!("Recorder {}p ({})", height, id),
                EgressType::RTMPForwarder { id, destination } => {
                    format!("RTMPForwarder {} ({})", destination, id)
                }
                EgressType::Moq { id } => format!("MoQ ({})", id),
            };

            writeln!(f, "{} {}", egress_prefix, egress_name)?;

            // Show variant groups for this egress
            let group_count = egress.variants.len();
            for (j, group) in egress.variants.iter().enumerate() {
                let is_last_group = j == group_count - 1;
                let group_prefix = if is_last_group {
                    "└──"
                } else {
                    "├──"
                };
                let variant_prefix = if is_last_group { "   " } else { "│  " };

                writeln!(f, "{} {} Group ({})", child_prefix, group_prefix, group.id)?;

                // Show variants in this group
                let mut variant_entries = Vec::new();
                if let Some(video_id) = &group.video {
                    if let Some(v) = self.variants.iter().find(|v| &v.id() == video_id) {
                        variant_entries.push(format!("{}", v));
                    } else {
                        variant_entries.push(format!("Video: {}", video_id));
                    }
                }
                if let Some(audio_id) = &group.audio {
                    if let Some(a) = self.variants.iter().find(|v| &v.id() == audio_id) {
                        variant_entries.push(format!("{}", a));
                    } else {
                        variant_entries.push(format!("Audio: {}", audio_id));
                    }
                }
                if let Some(sub_id) = &group.subtitle {
                    variant_entries.push(format!("Subtitle: {}", sub_id));
                }

                let entry_count = variant_entries.len();
                for (k, entry) in variant_entries.iter().enumerate() {
                    let is_last_entry = k == entry_count - 1;
                    let entry_prefix = if is_last_entry {
                        "└──"
                    } else {
                        "├──"
                    };
                    writeln!(
                        f,
                        "{} {} {} {}",
                        child_prefix, variant_prefix, entry_prefix, entry
                    )?;
                }
            }
        }
        Ok(())
    }
}
