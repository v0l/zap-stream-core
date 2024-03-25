use std::collections::HashMap;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_frame_alloc, av_packet_unref, AVCodec, avcodec_alloc_context3,
    avcodec_find_decoder, avcodec_free_context, avcodec_open2, avcodec_parameters_to_context,
    avcodec_receive_frame, avcodec_send_packet, AVCodecContext, AVERROR, AVERROR_EOF, AVPacket, AVStream,
};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::pipeline::PipelinePayload;

struct CodecContext {
    pub context: *mut AVCodecContext,
    pub codec: *const AVCodec,
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        unsafe {
            avcodec_free_context(&mut self.context);
        }
    }
}

pub struct Decoder {
    chan_in: UnboundedReceiver<PipelinePayload>,
    chan_out: broadcast::Sender<PipelinePayload>,
    codecs: HashMap<i32, CodecContext>,
}

unsafe impl Send for Decoder {}

unsafe impl Sync for Decoder {}

impl Decoder {
    pub fn new(
        chan_in: UnboundedReceiver<PipelinePayload>,
        chan_out: broadcast::Sender<PipelinePayload>,
    ) -> Self {
        Self {
            chan_in,
            chan_out,
            codecs: HashMap::new(),
        }
    }

    pub unsafe fn decode_pkt(&mut self, pkt: *mut AVPacket) -> Result<usize, Error> {
        let stream_index = (*pkt).stream_index as i32;
        let stream = (*pkt).opaque as *mut AVStream;
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

        if !self.codecs.contains_key(&stream_index) {
            let codec = avcodec_find_decoder((*codec_par).codec_id);
            if codec == ptr::null_mut() {
                return Err(Error::msg("Failed to find codec"));
            }
            let mut context = avcodec_alloc_context3(ptr::null());
            if context == ptr::null_mut() {
                return Err(Error::msg("Failed to alloc context"));
            }
            if avcodec_parameters_to_context(context, codec_par) != 0 {
                return Err(Error::msg("Failed to copy codec parameters to context"));
            }
            if avcodec_open2(context, codec, ptr::null_mut()) < 0 {
                return Err(Error::msg("Failed to open codec"));
            }
            self.codecs
                .insert(stream_index, CodecContext { context, codec });
        }
        if let Some(ctx) = self.codecs.get_mut(&stream_index) {
            let mut ret = -1;
            ret = avcodec_send_packet(ctx.context, pkt);
            av_packet_unref(pkt);
            if ret < 0 {
                return Err(Error::msg(format!("Failed to decode packet {}", ret)));
            }

            let mut frames = 0;
            while ret >= 0 {
                let frame = av_frame_alloc();
                ret = avcodec_receive_frame(ctx.context, frame);
                if ret < 0 {
                    if ret == AVERROR_EOF || ret == AVERROR(libc::EAGAIN) {
                        break;
                    }
                    return Err(Error::msg(format!("Failed to decode {}", ret)));
                }
                (*frame).time_base = (*pkt).time_base;
                (*frame).opaque = stream as *mut libc::c_void;
                self.chan_out.send(PipelinePayload::AvFrame(frame))?;
                frames += 1;
            }
            return Ok(frames);
        }
        Ok(0)
    }

    pub fn process(&mut self) -> Result<usize, Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            return if let PipelinePayload::AvPacket(pkt) = pkg {
                unsafe {
                    let frames = self.decode_pkt(pkt)?;
                    Ok(frames)
                }
            } else {
                Err(Error::msg("Payload not supported"))
            };
        }
        Ok(0)
    }
}
