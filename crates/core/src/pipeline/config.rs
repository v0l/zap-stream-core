use crate::egress::{EgressConfig, EgressType};
use crate::ingress::IngressInfo;
use crate::variant::VariantStream;
use std::fmt::{Display, Formatter};

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

impl PipelineConfig {
    /// Are we transcoding any video or audio variants
    pub fn is_transcoding(&self) -> bool {
        self.variants.iter().any(|v| match v {
            VariantStream::Video(_) | VariantStream::Audio(_) => true,
            _ => false,
        })
    }

    pub fn is_transcoding_src(&self, src_index: usize) -> bool {
        self.variants.iter().any(|var| match var {
            VariantStream::Video(v) if v.src_index == src_index => true,
            VariantStream::Audio(v) if v.src_index == src_index => true,
            _ => false,
        })
    }
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
                    let mut usplit = destination.split('/').collect::<Vec<&str>>();
                    usplit.pop();
                    usplit.push("<stream-key>");
                    format!("RTMPForwarder {} ({})", usplit.join("/"), id)
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
