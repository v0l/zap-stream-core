use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_packet_alloc, av_packet_free, av_packet_rescale_ts, avcodec_alloc_context3,
    avcodec_find_encoder, avcodec_open2, avcodec_receive_packet, avcodec_send_frame, AVCodec,
    AVCodecContext, AVFrame, AVRational, AVERROR,
};
use libc::EAGAIN;

use crate::pipeline::{AVFrameSource, AVPacketSource, PipelinePayload, PipelineProcessor};
use crate::return_ffmpeg_error;
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::video::VideoVariant;
use crate::variant::{EncodedStream, StreamMapping};

pub struct VideoEncoder {
    variant: VideoVariant,
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    pts: i64,
}

unsafe impl Send for VideoEncoder {}

unsafe impl Sync for VideoEncoder {}

impl VideoEncoder {
    pub fn new(variant: VideoVariant) -> Self {
        Self {
            ctx: ptr::null_mut(),
            codec: ptr::null(),
            variant,
            pts: 0,
        }
    }

    unsafe fn setup_encoder(&mut self) -> Result<Option<PipelinePayload>, Error> {
        if !self.ctx.is_null() {
            return Ok(None);
        }

        let codec = self.variant.codec;
        let encoder = avcodec_find_encoder(transmute(codec as i32));
        if encoder.is_null() {
            return Err(Error::msg("Encoder not found"));
        }

        let ctx = avcodec_alloc_context3(encoder);
        if ctx.is_null() {
            return Err(Error::msg("Failed to allocate encoder context"));
        }

        self.variant.to_codec_context(ctx);

        let ret = avcodec_open2(ctx, encoder, ptr::null_mut());
        return_ffmpeg_error!(ret);

        self.ctx = ctx;
        self.codec = encoder;
        Ok(Some(PipelinePayload::EncoderInfo(self.variant.id(), ctx)))
    }

    unsafe fn process_frame(
        &mut self,
        frame: *mut AVFrame,
        in_tb: &AVRational,
    ) -> Result<Vec<PipelinePayload>, Error> {
        let mut pkgs = Vec::new();

        if let Some(ei) = self.setup_encoder()? {
            pkgs.push(ei);
        }

        (*frame).pts = self.pts;
        self.pts += (*frame).duration;

        let mut ret = avcodec_send_frame(self.ctx, frame);
        if ret < 0 && ret != AVERROR(EAGAIN) {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        while ret > 0 || ret == AVERROR(EAGAIN) {
            let mut pkt = av_packet_alloc();
            ret = avcodec_receive_packet(self.ctx, pkt);
            if ret != 0 {
                av_packet_free(&mut pkt);
                if ret == AVERROR(EAGAIN) {
                    break;
                }
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            //set_encoded_pkt_timing(self.ctx, pkt, in_tb, &mut self.pts, &self.variant);
            av_packet_rescale_ts(pkt, *in_tb, self.variant.time_base());
            //dump_pkt_info(pkt);
            pkgs.push(PipelinePayload::AvPacket(
                pkt,
                AVPacketSource::Encoder(self.variant.id()),
            ));
        }

        Ok(pkgs)
    }
}

impl PipelineProcessor for VideoEncoder {
    fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error> {
        match pkg {
            PipelinePayload::AvFrame(frm, ref src) => unsafe {
                let (in_stream, idx) = match src {
                    AVFrameSource::Decoder(s) => (*s, (*(*s)).index as usize),
                    AVFrameSource::None(s) => (ptr::null_mut(), *s),
                    _ => return Err(Error::msg(format!("Cannot process frame from: {:?}", src))),
                };
                if self.variant.src_index() == idx {
                    let tb = if in_stream.is_null() {
                        self.variant.time_base()
                    } else {
                        (*in_stream).time_base
                    };
                    self.process_frame(frm, &tb)
                } else {
                    Ok(vec![])
                }
            },
            PipelinePayload::Flush => unsafe {
                self.process_frame(ptr::null_mut(), &AVRational { num: 0, den: 1 })
            },
            _ => Err(Error::msg("Payload not supported")),
        }
    }
}
