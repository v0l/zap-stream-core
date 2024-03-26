use std::ffi::CStr;
use std::fmt::{Display, Formatter};
use std::mem::transmute;

use ffmpeg_sys_next::{avcodec_get_name, AVRational};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VariantStream {
    /// Video stream mapping
    Video(VideoVariant),
    /// Audio stream mapping
    Audio(AudioVariant),
}

impl Display for VariantStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantStream::Video(v) => write!(f, "{}", v),
            VariantStream::Audio(a) => write!(f, "{}", a),
        }
    }
}

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoVariant {
    /// Unique ID of this variant
    pub id: Uuid,

    /// Source video stream to use for this variant
    pub src_index: usize,

    /// Index of this variant in the output
    pub dst_index: usize,

    /// Width of this video stream
    pub width: u16,

    /// Height of this video stream
    pub height: u16,

    /// FPS for this stream
    pub fps: u16,

    /// Bitrate of this stream
    pub bitrate: u64,

    /// AVCodecID
    pub codec: usize,

    /// Codec profile
    pub profile: usize,

    /// Codec level
    pub level: usize,

    /// Keyframe interval in seconds
    pub keyframe_interval: u16,
}

impl Display for VideoVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Video #{}->{}: {}, {}x{}, {}fps, {}kbps",
            self.src_index,
            self.dst_index,
            unsafe {
                CStr::from_ptr(avcodec_get_name(transmute(self.codec as i32)))
                    .to_str()
                    .unwrap()
            },
            self.width,
            self.height,
            self.fps,
            self.bitrate / 1000
        )
    }
}

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioVariant {
    /// Unique ID of this variant
    pub id: Uuid,

    /// Source video stream to use for this variant
    pub src_index: usize,

    /// Index of this variant in the output
    pub dst_index: usize,

    /// Bitrate of this stream
    pub bitrate: u64,

    /// AVCodecID
    pub codec: usize,

    /// Number of channels
    pub channels: u16,

    /// Sample rate
    pub sample_rate: usize,

    /// Sample format as ffmpeg sample format string
    pub sample_fmt: String,
}

impl Display for AudioVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Audio #{}->{}: {}, {}kbps",
            self.src_index,
            self.dst_index,
            unsafe {
                CStr::from_ptr(avcodec_get_name(transmute(self.codec as i32)))
                    .to_str()
                    .unwrap()
            },
            self.bitrate / 1000
        )
    }
}

impl VariantStream {
    pub fn id(&self) -> Uuid {
        match self {
            VariantStream::Video(v) => v.id,
            VariantStream::Audio(v) => v.id,
        }
    }

    pub fn src_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.src_index,
            VariantStream::Audio(v) => v.src_index,
        }
    }

    pub fn dst_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.dst_index,
            VariantStream::Audio(v) => v.dst_index,
        }
    }

    pub fn time_base(&self) -> AVRational {
        match &self {
            VariantStream::Video(vv) => vv.time_base(),
            VariantStream::Audio(va) => va.time_base(),
        }
    }
}

impl VideoVariant {
    pub fn time_base(&self) -> AVRational {
        AVRational {
            num: 1,
            den: 90_000,
        }
    }
}

impl AudioVariant {
    pub fn time_base(&self) -> AVRational {
        AVRational {
            num: 1,
            den: self.sample_rate as libc::c_int,
        }
    }
}
