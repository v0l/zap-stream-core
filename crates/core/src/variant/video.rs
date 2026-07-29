use crate::egress::EncoderParam;
use crate::ingress::IngressStream;
use crate::map_codec_id;
use anyhow::{Result, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AV_CODEC_FLAG_GLOBAL_HEADER, AV_CODEC_FLAG_LOW_DELAY, AVColorRange, AVColorSpace, AVPixelFormat,
    AVRational, av_color_range_from_name, av_color_space_from_name, av_get_pix_fmt, av_q2d,
    avcodec_find_encoder,
};
use ffmpeg_rs_raw::{Encoder, cstr, free_cstr, get_ffmpeg_error_msg, rstr};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use tracing::warn;
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
    /// Video color space
    pub color_space: String,
    /// Video color range
    pub color_range: String,
    /// Scale mode to use when resizing frames
    pub scale_mode: Option<ScaleMode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum ScaleMode {
    #[default]
    Default,
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
                EncoderParam::ColorRange { name } => {
                    self.color_range = name.clone();
                }
                EncoderParam::ColorSpace { name } => {
                    self.color_space = name.clone();
                }
                _ => {}
            }
        }
    }

    /// The level used to configure the encoder: whatever the config asked for, or the
    /// lowest level the content actually needs when the config leaves it unset.
    pub fn resolved_level(&self) -> u32 {
        if self.level > 0 {
            self.level
        } else {
            self.derive_h264_level()
        }
    }

    /// Lowest H.264 level (Annex A, Table A-1) whose frame size, macroblock rate and
    /// bitrate limits admit this variant.
    ///
    /// The level is advertised in the HLS codec attribute and players gate decoder
    /// selection on it, so pinning a high level makes a device reject a rendition it
    /// could decode easily.
    pub fn derive_h264_level(&self) -> u32 {
        // (level_idc, MaxMBPS, MaxFS in macroblocks, MaxBR in units of 1200 bit/s)
        // MaxBR is scaled by cpbBrVclFactor, 1200 for Baseline/Main/Extended.
        const LEVELS: [(u32, u64, u64, u64); 19] = [
            (10, 1_485, 99, 64),
            (11, 3_000, 396, 192),
            (12, 6_000, 396, 384),
            (13, 11_880, 396, 768),
            (20, 11_880, 396, 2_000),
            (21, 19_800, 792, 4_000),
            (22, 20_250, 1_620, 4_000),
            (30, 40_500, 1_620, 10_000),
            (31, 108_000, 3_600, 14_000),
            (32, 216_000, 5_120, 20_000),
            (40, 245_760, 8_192, 20_000),
            (41, 245_760, 8_192, 50_000),
            (42, 522_240, 8_704, 50_000),
            (50, 589_824, 22_080, 135_000),
            (51, 983_040, 36_864, 240_000),
            (52, 2_073_600, 36_864, 240_000),
            (60, 4_177_920, 139_264, 240_000),
            (61, 8_355_840, 139_264, 480_000),
            (62, 16_711_680, 139_264, 800_000),
        ];

        let mbs = self.width.div_ceil(16) as u64 * self.height.div_ceil(16) as u64;
        let fps = if self.fps > 0.0 { self.fps } else { 30.0 };
        let mbps = (mbs as f32 * fps).ceil() as u64;

        LEVELS
            .iter()
            .find(|(_, max_mbps, max_fs, max_br)| {
                mbs <= *max_fs && mbps <= *max_mbps && self.bitrate <= *max_br * 1200
            })
            .map(|(level, ..)| *level)
            // nothing in the table fits, ask for the highest level rather than none
            .unwrap_or(62)
    }

    /// Apply any modifications to the config based on the ingress stream which is used for this variant
    pub fn patch_for_ingress(&mut self, ingress: &IngressStream) {
        // if the ingress stream is a different size or pixel format, apply scale mode.
        // An unknown ingress pixel format must not panic; treat it as a mismatch so
        // the scaler is configured (which will convert or fail gracefully later).
        if ingress.width as u16 != self.width
            || ingress.height as u16 != self.height
            || ingress
                .pixel_format_name()
                .map(|n| n != self.pixel_format)
                .unwrap_or(true)
        {
            self.scale_mode = Some(ScaleMode::Default);
        }
    }

    pub fn pixel_format_id(&self) -> Result<i32> {
        unsafe {
            let c_fmt = cstr!(self.pixel_format.as_str());
            let fmt = av_get_pix_fmt(c_fmt);
            free_cstr!(c_fmt);
            if fmt == AVPixelFormat::NONE {
                bail!("Invalid pixel format {}", self.pixel_format);
            }
            Ok(fmt.0 as _)
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
                // frames with pict_type=I forced by the pipeline (time-based
                // keyframe interval) must become IDR frames, otherwise HLS
                // segments can't split on them
                opt.insert("forced-idr".to_string(), "1".to_string());
            }

            let enc = Encoder::new_with_codec(encoder)?
                .with_bitrate(self.bitrate as _)
                .with_width(self.width as _)
                .with_height(self.height as _)
                .with_pix_fmt(transmute(self.pixel_format_id()?))
                .with_profile(self.profile as _)
                .with_level(self.resolved_level() as _)
                .with_framerate(self.fps)?
                .with_options(|ctx| {
                    (*ctx).gop_size = self.gop as _;
                    (*ctx).keyint_min = self.gop as _;
                    (*ctx).max_b_frames = self.max_b_frames as _;
                    (*ctx).flags |= AV_CODEC_FLAG_LOW_DELAY as i32;

                    if !self.color_space.is_empty() {
                        let csn = cstr!(self.color_space.as_str());
                        let cs = av_color_space_from_name(csn);
                        free_cstr!(csn);
                        if cs < 0 {
                            warn!(
                                "Color space not found {}, {}",
                                self.color_space,
                                get_ffmpeg_error_msg(cs)
                            );
                        } else {
                            (*ctx).colorspace = transmute(cs);
                        }
                    } else {
                        (*ctx).colorspace = AVColorSpace::BT709;
                    }

                    if !self.color_range.is_empty() {
                        let csn = cstr!(self.color_range.as_str());
                        let cr = av_color_range_from_name(csn);
                        free_cstr!(csn);
                        if cr < 0 {
                            warn!(
                                "Color range not found {}, {}",
                                self.color_range,
                                get_ffmpeg_error_msg(cr)
                            );
                        } else {
                            (*ctx).color_range = transmute(cr);
                        }
                    } else {
                        (*ctx).color_range = AVColorRange::MPEG;
                    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(width: u16, height: u16, fps: f32, bitrate: u64) -> VideoVariant {
        VideoVariant {
            width,
            height,
            fps,
            bitrate,
            ..Default::default()
        }
    }

    #[test]
    fn derives_level_from_the_ladder() {
        assert_eq!(
            variant(1920, 1080, 23.976, 8_000_000).derive_h264_level(),
            40
        );
        assert_eq!(
            variant(1280, 720, 23.976, 4_000_000).derive_h264_level(),
            31
        );
        assert_eq!(variant(854, 480, 23.976, 1_500_000).derive_h264_level(), 30);
    }

    #[test]
    fn frame_rate_and_bitrate_raise_the_level() {
        // same frame size, 60fps exceeds the macroblock rate of 3.1
        assert_eq!(variant(1280, 720, 60.0, 4_000_000).derive_h264_level(), 32);
        // same frame size and rate, bitrate exceeds MaxBR of 3.1 (16.8Mb/s)
        assert_eq!(
            variant(1280, 720, 23.976, 20_000_000).derive_h264_level(),
            32
        );
    }

    #[test]
    fn handles_4k_and_missing_frame_rate() {
        assert_eq!(
            variant(3840, 2160, 30.0, 12_000_000).derive_h264_level(),
            51
        );
        // an unknown frame rate must not divide by zero or pick level 1
        assert_eq!(variant(1920, 1080, 0.0, 8_000_000).derive_h264_level(), 40);
    }

    #[test]
    fn configured_level_wins_over_derivation() {
        let mut v = variant(854, 480, 23.976, 1_500_000);
        assert_eq!(v.resolved_level(), 30);
        v.level = 51;
        assert_eq!(v.resolved_level(), 51);
    }
}
