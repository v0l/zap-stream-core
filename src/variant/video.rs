use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{avcodec_get_name, AVCodecID};
use ffmpeg_rs_raw::{rstr, Encoder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::intrinsics::transmute;
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

    /// AVCodecID
    pub codec: usize,

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
            "Video #{}->{}: {}, {}x{}, {}fps, {}kbps",
            self.mapping.src_index,
            self.mapping.dst_index,
            unsafe { rstr!(avcodec_get_name(transmute(self.codec as i32))) },
            self.width,
            self.height,
            self.fps,
            self.bitrate / 1000
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

impl TryInto<Encoder> for &VideoVariant {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Encoder, Self::Error> {
        unsafe {
            let mut opt = HashMap::new();
            if self.codec == transmute::<AVCodecID, u32>(AV_CODEC_ID_H264) as usize {
                opt.insert("preset".to_string(), "fast".to_string());
                //opt.insert("tune".to_string(), "zerolatency".to_string());
            }
            let enc = Encoder::new(transmute(self.codec as u32))?
                .with_bitrate(self.bitrate as _)
                .with_width(self.width as _)
                .with_height(self.height as _)
                .with_pix_fmt(transmute(self.pixel_format))
                .with_profile(transmute(self.profile as i32))
                .with_level(transmute(self.level as i32))
                .with_framerate(self.fps)
                .with_options(|ctx| {
                    (*ctx).gop_size = self.keyframe_interval as _;
                    (*ctx).keyint_min = self.keyframe_interval as _;
                    (*ctx).max_b_frames = 3;
                    (*ctx).colorspace = AVCOL_SPC_BT709;
                })
                .open(Some(opt))?;

            Ok(enc)
        }
    }
}
