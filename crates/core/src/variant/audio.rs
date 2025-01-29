use ffmpeg_rs_raw::ffmpeg_sys_the_third::av_get_sample_fmt;
use ffmpeg_rs_raw::{cstr, Encoder};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

use crate::variant::{StreamMapping, VariantMapping};

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioVariant {
    /// Id, Src, Dst
    pub mapping: VariantMapping,

    /// Bitrate of this stream
    pub bitrate: u64,

    /// Codec name
    pub codec: String,

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
            self.mapping.src_index,
            self.mapping.dst_index,
            self.codec,
            self.bitrate / 1000
        )
    }
}
impl StreamMapping for AudioVariant {
    fn id(&self) -> Uuid {
        self.mapping.id
    }
    fn src_index(&self) -> usize {
        self.mapping.src_index
    }

    fn dst_index(&self) -> usize {
        self.mapping.dst_index
    }

    fn set_dst_index(&mut self, dst: usize) {
        self.mapping.dst_index = dst;
    }

    fn group_id(&self) -> usize {
        self.mapping.group_id
    }
}

impl TryInto<Encoder> for &AudioVariant {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Encoder, Self::Error> {
        unsafe {
            let enc = Encoder::new_with_name(&self.codec)?
                .with_sample_rate(self.sample_rate as _)?
                .with_bitrate(self.bitrate as _)
                .with_default_channel_layout(self.channels as _)
                .with_sample_format(av_get_sample_fmt(cstr!(self.sample_fmt.as_bytes())))
                .open(None)?;

            Ok(enc)
        }
    }
}
