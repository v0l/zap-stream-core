use std::ffi::CStr;
use std::fmt::{Display, Formatter};
use std::intrinsics::transmute;

use ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_sys_next::{
    av_opt_set, avcodec_find_encoder, avcodec_get_name, AVCodec, AVCodecContext, AVCodecParameters,
    AVRational, AVStream,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::variant::{EncodedStream, StreamMapping, VariantMapping};

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoVariant {
    /// Id, Src, Dst
    pub mapping: VariantMapping,

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

    unsafe fn to_stream(&self, stream: *mut AVStream) {
        (*stream).time_base = self.time_base();
        (*stream).avg_frame_rate = AVRational {
            num: self.fps as libc::c_int,
            den: 1,
        };
        (*stream).r_frame_rate = AVRational {
            num: self.fps as libc::c_int,
            den: 1,
        };
        self.to_codec_params((*stream).codecpar);
    }
}

impl EncodedStream for VideoVariant {
    fn time_base(&self) -> AVRational {
        AVRational {
            num: 1,
            den: 90_000,
        }
    }

    unsafe fn get_codec(&self) -> *const AVCodec {
        avcodec_find_encoder(transmute(self.codec as u32))
    }

    unsafe fn to_codec_context(&self, ctx: *mut AVCodecContext) {
        let codec = self.get_codec();
        (*ctx).codec_id = (*codec).id;
        (*ctx).codec_type = (*codec).type_;
        (*ctx).time_base = self.time_base();
        (*ctx).bit_rate = self.bitrate as i64;
        (*ctx).width = self.width as libc::c_int;
        (*ctx).height = self.height as libc::c_int;
        (*ctx).level = self.level as libc::c_int;
        (*ctx).profile = self.profile as libc::c_int;
        (*ctx).framerate = AVRational {
            num: self.fps as libc::c_int,
            den: 1,
        };

        (*ctx).gop_size = self.keyframe_interval as libc::c_int;
        (*ctx).keyint_min = self.keyframe_interval as libc::c_int;
        (*ctx).max_b_frames = 3;
        (*ctx).pix_fmt = AV_PIX_FMT_YUV420P;
        (*ctx).colorspace = AVCOL_SPC_BT709;
        if (*codec).id == AV_CODEC_ID_H264 {
            av_opt_set(
                (*ctx).priv_data,
                "preset\0".as_ptr() as *const libc::c_char,
                "fast\0".as_ptr() as *const libc::c_char,
                0,
            );
            av_opt_set(
                (*ctx).priv_data,
                "tune\0".as_ptr() as *const libc::c_char,
                "zerolatency\0".as_ptr() as *const libc::c_char,
                0,
            );
        }
    }

    unsafe fn to_codec_params(&self, params: *mut AVCodecParameters) {
        let codec = self.get_codec();
        (*params).codec_id = (*codec).id;
        (*params).codec_type = (*codec).type_;
        (*params).height = self.height as libc::c_int;
        (*params).width = self.width as libc::c_int;
        (*params).format = AV_PIX_FMT_YUV420P as i32;
        (*params).framerate = AVRational {
            num: self.fps as libc::c_int,
            den: 1,
        };
        (*params).bit_rate = self.bitrate as i64;
        (*params).color_space = AVCOL_SPC_BT709;
        (*params).level = self.level as libc::c_int;
        (*params).profile = self.profile as libc::c_int;
    }
}
