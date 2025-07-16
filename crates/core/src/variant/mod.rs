use crate::variant::audio::AudioVariant;
use crate::variant::mapping::VariantMapping;
use crate::variant::video::VideoVariant;
use anyhow::Error;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

pub mod audio;
pub mod mapping;
pub mod video;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VariantStream {
    /// Video stream mapping
    Video(VideoVariant),
    /// Audio stream mapping
    Audio(AudioVariant),
    Subtitle(VariantMapping),
    /// Copy stream src<>dst stream
    CopyVideo(VideoVariant),
    /// Copy stream src<>dst stream
    CopyAudio(AudioVariant),
}

impl StreamMapping for VariantStream {
    fn id(&self) -> Uuid {
        match self {
            VariantStream::Video(v) => v.id(),
            VariantStream::Audio(v) => v.id(),
            VariantStream::Subtitle(v) => v.id(),
            VariantStream::CopyAudio(v) => v.id(),
            VariantStream::CopyVideo(v) => v.id(),
        }
    }

    fn src_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.src_index(),
            VariantStream::Audio(v) => v.src_index(),
            VariantStream::Subtitle(v) => v.src_index(),
            VariantStream::CopyAudio(v) => v.src_index(),
            VariantStream::CopyVideo(v) => v.src_index(),
        }
    }

    fn dst_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.dst_index(),
            VariantStream::Audio(v) => v.dst_index(),
            VariantStream::Subtitle(v) => v.dst_index(),
            VariantStream::CopyAudio(v) => v.dst_index(),
            VariantStream::CopyVideo(v) => v.dst_index(),
        }
    }

    fn set_dst_index(&mut self, dst: usize) {
        match self {
            VariantStream::Video(v) => v.set_dst_index(dst),
            VariantStream::Audio(v) => v.set_dst_index(dst),
            VariantStream::Subtitle(v) => v.set_dst_index(dst),
            VariantStream::CopyAudio(v) => v.set_dst_index(dst),
            VariantStream::CopyVideo(v) => v.set_dst_index(dst),
        }
    }

    fn group_id(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.group_id(),
            VariantStream::Audio(v) => v.group_id(),
            VariantStream::Subtitle(v) => v.group_id(),
            VariantStream::CopyAudio(v) => v.group_id(),
            VariantStream::CopyVideo(v) => v.group_id(),
        }
    }
}

impl Display for VariantStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantStream::Video(v) => write!(f, "{}", v),
            VariantStream::Audio(a) => write!(f, "{}", a),
            VariantStream::Subtitle(s) => write!(f, "{}", s),
            VariantStream::CopyVideo(c) => write!(f, "{}", c),
            VariantStream::CopyAudio(c) => write!(f, "{}", c),
        }
    }
}

pub trait StreamMapping {
    fn id(&self) -> Uuid;
    fn src_index(&self) -> usize;
    fn dst_index(&self) -> usize;
    fn set_dst_index(&mut self, dst: usize);
    fn group_id(&self) -> usize;
}

/// Find a stream by ID in a vec of streams
pub fn find_stream<'a>(
    config: &'a Vec<VariantStream>,
    id: &Uuid,
) -> Result<&'a VariantStream, Error> {
    config
        .iter()
        .find(|x| match x {
            VariantStream::Video(v) => v.id() == *id,
            VariantStream::Audio(a) => a.id() == *id,
            VariantStream::Subtitle(v) => v.id() == *id,
            VariantStream::CopyVideo(c) => c.id() == *id,
            VariantStream::CopyAudio(c) => c.id() == *id,
        })
        .ok_or(Error::msg("Variant does not exist"))
}
