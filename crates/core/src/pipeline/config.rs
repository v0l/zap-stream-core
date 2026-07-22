use crate::egress::{EgressConfig, EgressType};
use crate::ingress::IngressInfo;
use crate::pipeline::PipelinePlugin;
use crate::variant::VariantStream;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

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
    /// Runtime plugins enabled for this stream pipeline
    pub plugins: Vec<Arc<dyn PipelinePlugin>>,
}

impl PipelineConfig {
    /// Is decoding the input stream required
    pub fn should_decode(&self) -> bool {
        self.variants.iter().any(|v| match v {
            VariantStream::Video(_) | VariantStream::Audio(_) | VariantStream::Plugin { .. } => {
                true
            }
            _ => false,
        })
    }

    pub fn is_transcoding_src(&self, src_index: usize) -> bool {
        self.variants.iter().any(|var| match var {
            VariantStream::Video(v) => v.src_index == src_index,
            VariantStream::Audio(v) => v.src_index == src_index,
            // NOTE: bind with a distinct name; the previous pattern binding shadowed
            // the function argument (`src_index == src_index`), which was always true
            // and forced every stream through the decoder when any plugin was active
            VariantStream::Plugin {
                src_index: plugin_src,
                ..
            } => *plugin_src == src_index,
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Regression: Plugin variants used a shadowed pattern binding making
    /// is_transcoding_src always true for every stream index.
    #[test]
    fn is_transcoding_src_plugin_matches_only_its_source() {
        let cfg = PipelineConfig {
            variants: vec![VariantStream::Plugin {
                id: Uuid::new_v4(),
                name: "test".to_string(),
                src_index: 1,
            }],
            egress: vec![],
            ingress_info: IngressInfo {
                bitrate: 0,
                streams: vec![],
            },
            video_src: 0,
            audio_src: Some(1),
            plugins: vec![],
        };
        assert!(cfg.is_transcoding_src(1));
        assert!(!cfg.is_transcoding_src(0));
        assert!(!cfg.is_transcoding_src(2));
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
