use std::ffi::CStr;
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use std::ptr;

use ffmpeg_sys_next::{
    AV_CH_LAYOUT_STEREO, av_get_sample_fmt, av_opt_set, AVChannelLayout,
    AVChannelLayout__bindgen_ty_1, AVCodec, avcodec_find_encoder, avcodec_find_encoder_by_name, avcodec_get_name,
    AVCodecContext, AVCodecParameters, AVRational, AVStream,
};
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use ffmpeg_sys_next::AVCodecID::{AV_CODEC_ID_AAC, AV_CODEC_ID_H264};
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
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

impl VariantStreamType for VariantStream {
    fn id(&self) -> Uuid {
        match self {
            VariantStream::Video(v) => v.id,
            VariantStream::Audio(v) => v.id,
        }
    }

    fn src_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.src_index,
            VariantStream::Audio(v) => v.src_index,
        }
    }

    fn dst_index(&self) -> usize {
        match self {
            VariantStream::Video(v) => v.dst_index,
            VariantStream::Audio(v) => v.dst_index,
        }
    }

    fn time_base(&self) -> AVRational {
        match self {
            VariantStream::Video(v) => v.time_base(),
            VariantStream::Audio(v) => v.time_base(),
        }
    }

    unsafe fn get_codec(&self) -> *const AVCodec {
        match self {
            VariantStream::Video(v) => v.get_codec(),
            VariantStream::Audio(v) => v.get_codec(),
        }
    }

    unsafe fn to_codec_context(&self, ctx: *mut AVCodecContext) {
        match self {
            VariantStream::Video(v) => v.to_codec_context(ctx),
            VariantStream::Audio(v) => v.to_codec_context(ctx),
        }
    }

    unsafe fn to_codec_params(&self, params: *mut AVCodecParameters) {
        match self {
            VariantStream::Video(v) => v.to_codec_params(params),
            VariantStream::Audio(v) => v.to_codec_params(params),
        }
    }

    unsafe fn to_stream(&self, stream: *mut AVStream) {
        match self {
            VariantStream::Video(v) => v.to_stream(stream),
            VariantStream::Audio(v) => v.to_stream(stream),
        }
    }
}

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
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

pub trait VariantStreamType {
    fn id(&self) -> Uuid;
    fn src_index(&self) -> usize;
    fn dst_index(&self) -> usize;
    fn time_base(&self) -> AVRational;
    unsafe fn get_codec(&self) -> *const AVCodec;
    unsafe fn to_codec_context(&self, ctx: *mut AVCodecContext);
    unsafe fn to_codec_params(&self, params: *mut AVCodecParameters);
    unsafe fn to_stream(&self, stream: *mut AVStream);
}

impl VariantStreamType for VideoVariant {
    fn id(&self) -> Uuid {
        self.id
    }

    fn src_index(&self) -> usize {
        self.src_index
    }

    fn dst_index(&self) -> usize {
        self.dst_index
    }

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

        let key_frames = self.fps * self.keyframe_interval;
        (*ctx).gop_size = key_frames as libc::c_int;
        (*ctx).keyint_min = key_frames as libc::c_int;
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

impl VariantStreamType for AudioVariant {
    fn id(&self) -> Uuid {
        self.id
    }

    fn src_index(&self) -> usize {
        self.src_index
    }

    fn dst_index(&self) -> usize {
        self.dst_index
    }

    fn time_base(&self) -> AVRational {
        AVRational {
            num: 1,
            den: self.sample_rate as libc::c_int,
        }
    }

    unsafe fn get_codec(&self) -> *const AVCodec {
        if self.codec == AV_CODEC_ID_AAC as usize {
            avcodec_find_encoder_by_name("libfdk_aac\0".as_ptr() as *const libc::c_char)
        } else {
            avcodec_find_encoder(transmute(self.codec as u32))
        }
    }

    unsafe fn to_codec_context(&self, ctx: *mut AVCodecContext) {
        let codec = self.get_codec();
        (*ctx).codec_id = (*codec).id;
        (*ctx).codec_type = (*codec).type_;
        (*ctx).time_base = self.time_base();
        (*ctx).sample_fmt =
            av_get_sample_fmt(format!("{}\0", self.sample_fmt).as_ptr() as *const libc::c_char);
        (*ctx).bit_rate = self.bitrate as i64;
        (*ctx).sample_rate = self.sample_rate as libc::c_int;
        (*ctx).ch_layout = self.channel_layout();
    }

    unsafe fn to_codec_params(&self, params: *mut AVCodecParameters) {
        let codec = self.get_codec();
        (*params).codec_id = (*codec).id;
        (*params).codec_type = (*codec).type_;
        (*params).format =
            av_get_sample_fmt(format!("{}\0", self.sample_fmt).as_ptr() as *const libc::c_char)
                as libc::c_int;
        (*params).bit_rate = self.bitrate as i64;
        (*params).sample_rate = self.sample_rate as libc::c_int;
        (*params).ch_layout = self.channel_layout();
        (*params).frame_size = 1024; //TODO: fix this
    }

    unsafe fn to_stream(&self, stream: *mut AVStream) {
        (*stream).time_base = self.time_base();
        self.to_codec_params((*stream).codecpar);
    }
}

impl AudioVariant {
    fn channel_layout(&self) -> AVChannelLayout {
        AVChannelLayout {
            order: AV_CHANNEL_ORDER_NATIVE,
            nb_channels: 2,
            u: AVChannelLayout__bindgen_ty_1 {
                mask: AV_CH_LAYOUT_STEREO,
            },
            opaque: ptr::null_mut(),
        }
    }
}
