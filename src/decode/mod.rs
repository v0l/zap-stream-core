use std::collections::HashMap;
use std::ffi::CStr;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_sys_next::{
    av_frame_alloc, avcodec_alloc_context3, avcodec_find_decoder, avcodec_free_context,
    avcodec_get_name, avcodec_open2, avcodec_parameters_to_context, avcodec_receive_frame,
    avcodec_send_packet, AVCodec, AVCodecContext, AVPacket, AVERROR, AVERROR_EOF,
};

use crate::pipeline::{AVFrameSource, AVPacketSource, PipelinePayload};

struct CodecContext {
    pub context: *mut AVCodecContext,
    pub codec: *const AVCodec,
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        unsafe {
            avcodec_free_context(&mut self.context);
            self.codec = ptr::null_mut();
            self.context = ptr::null_mut();
        }
    }
}

pub struct Decoder {
    codecs: HashMap<i32, CodecContext>,
    pts: i64,
}

unsafe impl Send for Decoder {}

unsafe impl Sync for Decoder {}

impl Decoder {
    pub fn new() -> Self {
        Self {
            codecs: HashMap::new(),
            pts: 0,
        }
    }

    pub unsafe fn decode_pkt(
        &mut self,
        pkt: *mut AVPacket,
        src: &AVPacketSource,
    ) -> Result<Vec<PipelinePayload>, Error> {
        let stream_index = (*pkt).stream_index;
        let stream = match src {
            AVPacketSource::Demuxer(s) => *s,
            _ => return Err(Error::msg(format!("Cannot decode packet from: {:?}", src))),
        };

        assert_eq!(
            stream_index,
            (*stream).index,
            "Passed stream reference does not match stream_index of packet"
        );

        let codec_par = (*stream).codecpar;
        assert_ne!(
            codec_par,
            ptr::null_mut(),
            "Codec parameters are missing from stream"
        );

        if let std::collections::hash_map::Entry::Vacant(e) = self.codecs.entry(stream_index) {
            let codec = avcodec_find_decoder((*codec_par).codec_id);
            if codec.is_null() {
                return Err(Error::msg(format!(
                    "Failed to find codec: {}",
                    CStr::from_ptr(avcodec_get_name((*codec_par).codec_id))
                        .to_str()?
                )));
            }
            let context = avcodec_alloc_context3(ptr::null());
            if context.is_null() {
                return Err(Error::msg("Failed to alloc context"));
            }
            if avcodec_parameters_to_context(context, (*stream).codecpar) != 0 {
                return Err(Error::msg("Failed to copy codec parameters to context"));
            }
            (*context).pkt_timebase = (*stream).time_base;
            if avcodec_open2(context, codec, ptr::null_mut()) < 0 {
                return Err(Error::msg("Failed to open codec"));
            }
            e.insert(CodecContext { context, codec });
        }
        if let Some(ctx) = self.codecs.get_mut(&stream_index) {
            let mut ret = avcodec_send_packet(ctx.context, pkt);
            if ret < 0 {
                return Err(Error::msg(format!("Failed to decode packet {}", ret)));
            }

            let mut pkgs = Vec::new();
            while ret >= 0 {
                let frame = av_frame_alloc();
                ret = avcodec_receive_frame(ctx.context, frame);
                if ret < 0 {
                    if ret == AVERROR_EOF || ret == AVERROR(libc::EAGAIN) {
                        break;
                    }
                    return Err(Error::msg(format!("Failed to decode {}", ret)));
                }

                (*frame).pict_type = AV_PICTURE_TYPE_NONE; // encoder prints warnings
                pkgs.push(PipelinePayload::AvFrame(
                    frame,
                    AVFrameSource::Decoder(stream),
                ));
            }
            Ok(pkgs)
        } else {
            Ok(vec![])
        }
    }

    pub fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error> {
        if let PipelinePayload::AvPacket(pkt, ref src) = pkg {
            unsafe { self.decode_pkt(pkt, src) }
        } else {
            Err(Error::msg("Payload not supported"))
        }
    }
}
