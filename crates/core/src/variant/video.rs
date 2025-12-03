use crate::egress::EncoderParam;
use crate::map_codec_id;
use anyhow::{Result, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AV_CODEC_FLAG_GLOBAL_HEADER, AVRational, av_get_pix_fmt, av_q2d, avcodec_find_encoder, AV_CODEC_FLAG_LOW_DELAY};
use ffmpeg_rs_raw::{Encoder, cstr, free_cstr, rstr};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use uuid::Uuid;

/// Information related to variant streams
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoVariant {
    /// Unique ID of this variant
    pub id: Uuid,
    /// Source video stream to use for this variant
    pub src_index: usize,
    /// Width of this video stream
    pub width: u16,
    /// Height of this video stream
    pub height: u16,
    /// FPS for this stream
    pub fps: f32,
    /// Bitrate of this stream
    pub bitrate: u64,
    /// Codec name
    pub codec: String,
    /// Encoder preset name
    pub preset: Option<String>,
    /// Encoder tune param
    pub tune: Option<String>,
    /// Number of frames in a group
    pub gop: u32,
    /// Max number of B frames
    pub max_b_frames: u32,
    /// Codec profile (h264)
    pub profile: u32,
    /// Codec level (h264)
    pub level: u32,
    /// Video pixel format name
    pub pixel_format: String,
}

impl Display for VideoVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Video #{}: {}, {}x{}, {}fps, {}kbps ({})",
            self.src_index,
            self.codec,
            self.width,
            self.height,
            self.fps,
            self.bitrate / 1000,
            self.id
        )
    }
}

impl VideoVariant {
    pub fn apply_params(&mut self, params: &Vec<EncoderParam>) {
        for param in params {
            match param {
                EncoderParam::Width { value } => {
                    self.width = *value;
                }
                EncoderParam::Height { value } => {
                    self.height = *value;
                }
                EncoderParam::Bitrate { value } => {
                    self.bitrate = *value;
                }
                EncoderParam::Framerate { num, den } => {
                    self.fps = unsafe {
                        av_q2d(AVRational {
                            num: *num as _,
                            den: *den as _,
                        }) as _
                    };
                }
                EncoderParam::Preset { name } => {
                    self.preset = Some(name.clone());
                }
                EncoderParam::Tune { name } => {
                    self.tune = Some(name.clone());
                }
                EncoderParam::GOPSize { size } => {
                    self.gop = *size;
                }
                EncoderParam::MaxBFrames { size } => {
                    self.max_b_frames = *size;
                }
                EncoderParam::Profile { id } => {
                    self.profile = *id;
                }
                EncoderParam::Level { id } => {
                    self.level = *id;
                }
                EncoderParam::PixelFormat { name } => {
                    self.pixel_format = name.clone();
                }
                _ => {}
            }
        }
    }

    pub fn pixel_format_id(&self) -> Result<i32> {
        unsafe {
            let c_fmt = cstr!(self.pixel_format.as_str());
            let fmt = av_get_pix_fmt(c_fmt);
            free_cstr!(c_fmt);
            if fmt == AV_PIX_FMT_NONE {
                bail!("Invalid pixel format {}", self.pixel_format);
            }
            Ok(fmt as _)
        }
    }

    /// Create encoder with conditional GLOBAL_HEADER flag
    pub fn create_encoder(&self, need_global_header: bool) -> Result<Encoder> {
        unsafe {
            let mut opt = HashMap::new();
            let Some(codec_id) = map_codec_id(&self.codec) else {
                bail!("Could not find codec id for {}", &self.codec);
            };

            let encoder = avcodec_find_encoder(codec_id);
            if encoder.is_null() {
                bail!("No available encoder for codec {}", &self.codec);
            }

            let encoder_name = rstr!((*encoder).name);
            if encoder_name == "libx264" {
                if let Some(preset) = &self.preset {
                    opt.insert("preset".to_string(), preset.clone());
                }
                if let Some(tune) = &self.tune {
                    opt.insert("tune".to_string(), tune.clone());
                }
            }

            let enc = Encoder::new_with_codec(encoder)?
                .with_bitrate(self.bitrate as _)
                .with_width(self.width as _)
                .with_height(self.height as _)
                .with_pix_fmt(transmute(self.pixel_format_id()?))
                .with_profile(self.profile as _)
                .with_level(self.level as _)
                .with_framerate(self.fps)?
                .with_options(|ctx| {
                    (*ctx).gop_size = self.gop as _;
                    (*ctx).keyint_min = self.gop as _;
                    (*ctx).max_b_frames = self.max_b_frames as _;
                    (*ctx).colorspace = AVCOL_SPC_BT709;
                    (*ctx).flags |= AV_CODEC_FLAG_LOW_DELAY as i32;

                    // Set GLOBAL_HEADER flag for fMP4 HLS and recorder contexts
                    if need_global_header {
                        (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
                    }
                    // force timebase 90k tbn
                    (*ctx).time_base = AVRational {
                        num: 1,
                        den: 90_000,
                    }
                })
                .open(Some(opt))?;

            Ok(enc)
        }
    }
}
