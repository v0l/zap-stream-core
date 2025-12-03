use crate::egress::EncoderParam;
use crate::map_codec_id;
use anyhow::Result;
use anyhow::bail;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AV_CODEC_FLAG_GLOBAL_HEADER, AV_CODEC_FLAG_LOW_DELAY, av_get_sample_fmt, avcodec_find_encoder,
};
use ffmpeg_rs_raw::{Encoder, cstr, free_cstr};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use uuid::Uuid;

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioVariant {
    /// Unique ID of this variant
    pub id: Uuid,
    /// Source video stream to use for this variant
    pub src_index: usize,
    /// Bitrate of this stream
    pub bitrate: u64,
    /// Codec name
    pub codec: String,
    /// Audio sample format name
    pub sample_format: String,
    /// Audio samples per second
    pub sample_rate: u32,
    /// Audio channel count
    pub channels: u8,
}

impl Display for AudioVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Audio #{}: {}, {}kbps, {:.2} kHz, {}ch ({})",
            self.src_index,
            self.codec,
            self.bitrate / 1000,
            self.sample_rate as f32 / 1000.0,
            self.channels,
            self.id,
        )
    }
}

impl AudioVariant {
    pub fn apply_params(&mut self, params: &Vec<EncoderParam>) {
        for param in params {
            match param {
                EncoderParam::Bitrate { value } => {
                    self.bitrate = *value;
                }
                EncoderParam::SampleFormat { name } => {
                    self.sample_format = name.clone();
                }
                EncoderParam::SampleRate { size } => {
                    self.sample_rate = *size;
                }
                EncoderParam::AudioChannels { count } => {
                    self.channels = *count as u8;
                }
                _ => {}
            }
        }
    }

    pub fn sample_format_id(&self) -> Result<i32> {
        unsafe {
            let n_c = cstr!(self.sample_format.as_str());
            let id = av_get_sample_fmt(n_c);
            free_cstr!(n_c);
            if id == AV_SAMPLE_FMT_NONE {
                bail!("Sample format {} not supported", self.sample_format);
            }
            Ok(id as _)
        }
    }

    /// Create encoder with conditional GLOBAL_HEADER flag
    pub fn create_encoder(&self, need_global_header: bool) -> Result<Encoder> {
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
                .with_sample_format(transmute(self.sample_format_id()?))
                .with_options(|ctx| {
                    (*ctx).flags |= AV_CODEC_FLAG_LOW_DELAY as i32;
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
