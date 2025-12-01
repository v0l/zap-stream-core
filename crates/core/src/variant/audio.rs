use crate::map_codec_id;
use crate::variant::{StreamMapping, VariantMapping};
use anyhow::bail;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AV_CODEC_FLAG_GLOBAL_HEADER, av_get_sample_fmt, avcodec_find_encoder,
};
use ffmpeg_rs_raw::{Encoder, cstr};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

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
            "Audio #{}->{}: {}, {}kbps ({})",
            self.mapping.src_index,
            self.mapping.dst_index,
            self.codec,
            self.bitrate / 1000,
            self.mapping.id,
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

impl AudioVariant {
    /// Create encoder with conditional GLOBAL_HEADER flag
    pub fn create_encoder(&self, need_global_header: bool) -> Result<Encoder, anyhow::Error> {
        unsafe {
            let Some(codec_id) = map_codec_id(&self.codec) else {
                bail!("Could not find codec id for {}", &self.codec);
            };

            let encoder = avcodec_find_encoder(codec_id);
            if encoder.is_null() {
                bail!("No available encoder for codec {}", &self.codec);
            }
            let enc = Encoder::new_with_codec(encoder)?
                .with_sample_rate(self.sample_rate as _)?
                .with_bitrate(self.bitrate as _)
                .with_default_channel_layout(self.channels as _)
                .with_sample_format(av_get_sample_fmt(cstr!(self.sample_fmt.as_bytes())))
                .with_options(|ctx| {
                    // Set GLOBAL_HEADER flag for fMP4 HLS and recorder contexts
                    if need_global_header {
                        (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
                    }
                })
                .open(None)?;

            Ok(enc)
        }
    }
}
