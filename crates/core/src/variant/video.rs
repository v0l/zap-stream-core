use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AV_CODEC_FLAG_GLOBAL_HEADER;
use ffmpeg_rs_raw::Encoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use uuid::Uuid;

use crate::variant::{StreamMapping, VariantMapping};

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoVariant {
    /// Id, Src, Dst
    pub mapping: VariantMapping,

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

    /// Codec profile
    pub profile: usize,

    /// Codec level
    pub level: usize,

    /// Keyframe interval in frames
    pub keyframe_interval: u16,

    /// Pixel Format
    pub pixel_format: u32,
}

impl Display for VideoVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Video #{}->{}: {}, {}x{}, {}fps, {}kbps ({})",
            self.mapping.src_index,
            self.mapping.dst_index,
            self.codec,
            self.width,
            self.height,
            self.fps,
            self.bitrate / 1000,
            self.mapping.id
        )
    }
}

impl StreamMapping for VideoVariant {
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

impl VideoVariant {
    /// Create encoder with conditional GLOBAL_HEADER flag
    pub fn create_encoder(&self, need_global_header: bool) -> Result<Encoder, anyhow::Error> {
        unsafe {
            let mut opt = HashMap::new();
            if self.codec == "x264" || self.codec == "libx264" {
                opt.insert("preset".to_string(), "fast".to_string());
                //opt.insert("tune".to_string(), "zerolatency".to_string());
            }
            let enc = Encoder::new_with_name(&self.codec)?
                .with_bitrate(self.bitrate as _)
                .with_width(self.width as _)
                .with_height(self.height as _)
                .with_pix_fmt(transmute(self.pixel_format))
                .with_profile(transmute(self.profile as i32))
                .with_level(transmute(self.level as i32))
                .with_framerate(self.fps)?
                .with_options(|ctx| {
                    (*ctx).gop_size = self.keyframe_interval as _;
                    (*ctx).keyint_min = self.keyframe_interval as _;
                    (*ctx).max_b_frames = 3;
                    (*ctx).colorspace = AVCOL_SPC_BT709;
                    
                    // Set GLOBAL_HEADER flag for fMP4 HLS and recorder contexts
                    if need_global_header {
                        (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
                    }
                })
                .open(Some(opt))?;
            Ok(enc)
        }
    }
}

impl TryInto<Encoder> for &VideoVariant {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Encoder, Self::Error> {
        // Default behavior - no GLOBAL_HEADER for backward compatibility
        self.create_encoder(false)
    }
}
