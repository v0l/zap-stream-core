use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

mod audio;
mod video;

pub use audio::*;
pub use video::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VariantStream {
    /// Video stream mapping
    Video(VideoVariant),
    /// Audio stream mapping
    Audio(AudioVariant),
    Subtitle {
        /// Unique ID of this variant
        id: Uuid,
        /// Source video stream to use for this variant
        src_index: usize,
    },
    /// Copy stream src<>dst stream
    CopyVideo(VideoVariant),
    /// Copy stream src<>dst stream
    CopyAudio(AudioVariant),
    /// A variant created by a plugin
    Plugin {
        /// Unique ID of this variant
        id: Uuid,
        /// Name of the plugin
        name: String,
        /// Source video stream to use for this variant
        src_index: usize,
    },
}

impl VariantStream {
    pub fn id(&self) -> Uuid {
        match self {
            VariantStream::Video(v) => v.id,
            VariantStream::Audio(a) => a.id,
            VariantStream::Subtitle { id, .. } => *id,
            VariantStream::CopyVideo(v) => v.id,
            VariantStream::CopyAudio(a) => a.id,
            VariantStream::Plugin { id, .. } => *id,
        }
    }

    pub fn src_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.src_index,
            VariantStream::Audio(a) => a.src_index,
            VariantStream::Subtitle { src_index, .. } => *src_index,
            VariantStream::CopyVideo(v) => v.src_index,
            VariantStream::CopyAudio(v) => v.src_index,
            VariantStream::Plugin { src_index, .. } => *src_index,
        }
    }

    pub fn bitrate(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.bitrate as _,
            VariantStream::Audio(a) => a.bitrate as _,
            VariantStream::Subtitle { .. } => 0,
            VariantStream::CopyVideo(v) => v.bitrate as _,
            VariantStream::CopyAudio(a) => a.bitrate as _,
            VariantStream::Plugin { .. } => 0,
        }
    }
}

impl Display for VariantStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantStream::Video(v) => write!(f, "{}", v),
            VariantStream::Audio(a) => write!(f, "{}", a),
            VariantStream::Subtitle { id, src_index } => {
                write!(f, "Subtitle #{}->{}", src_index, id)
            }
            VariantStream::CopyVideo(c) => write!(f, "Copy {}", c),
            VariantStream::CopyAudio(c) => write!(f, "Copy {}", c),
            VariantStream::Plugin {
                name,
                src_index,
                id,
            } => write!(f, "{}#{} ({})", src_index, name, id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariantGroup {
    pub id: Uuid,
    pub video: Option<Uuid>,
    pub audio: Option<Uuid>,
    pub subtitle: Option<Uuid>,
}

impl Default for VariantGroup {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            video: None,
            audio: None,
            subtitle: None,
        }
    }
}
