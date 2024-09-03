use std::ffi::CStr;
use std::fmt::{Display, Formatter};
use std::intrinsics::transmute;
use std::ptr;

use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_AAC;
use ffmpeg_sys_next::{
    av_get_sample_fmt, avcodec_find_encoder, avcodec_find_encoder_by_name, avcodec_get_name,
    AVChannelLayout, AVChannelLayout__bindgen_ty_1, AVCodec, AVCodecContext, AVCodecParameters,
    AVRational, AVStream, AV_CH_LAYOUT_STEREO,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::variant::{EncodedStream, StreamMapping, VariantMapping};

/// Information related to variant streams for a given egress
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioVariant {
    /// Id, Src, Dst
    pub mapping: VariantMapping,

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
            self.mapping.src_index,
            self.mapping.dst_index,
            unsafe {
                CStr::from_ptr(avcodec_get_name(transmute(self.codec as i32)))
                    .to_str()
                    .unwrap()
            },
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

    unsafe fn to_stream(&self, stream: *mut AVStream) {
        (*stream).time_base = self.time_base();
        self.to_codec_params((*stream).codecpar);
    }
}

impl EncodedStream for AudioVariant {
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
        (*ctx).frame_size = 1024;
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
