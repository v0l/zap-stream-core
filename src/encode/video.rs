use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_packet_alloc, av_packet_free, av_packet_rescale_ts, AVCodec,
    avcodec_alloc_context3, avcodec_find_encoder, avcodec_open2, avcodec_receive_packet, avcodec_send_frame,
    AVCodecContext, AVERROR, AVFrame, AVRational,
};
use libc::EAGAIN;
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::{AVFrameSource, AVPacketSource, PipelinePayload, PipelineProcessor};
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::{VariantStreamType, VideoVariant};

pub struct VideoEncoder<T> {
    variant: VideoVariant,
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    chan_in: T,
    chan_out: UnboundedSender<PipelinePayload>,
    pts: i64,
}

unsafe impl<T> Send for VideoEncoder<T> {}

unsafe impl<T> Sync for VideoEncoder<T> {}

impl<TRecv> VideoEncoder<TRecv>
where
    TRecv: Rx<PipelinePayload>,
{
    pub fn new(
        chan_in: TRecv,
        chan_out: UnboundedSender<PipelinePayload>,
        variant: VideoVariant,
    ) -> Self {
        Self {
            ctx: ptr::null_mut(),
            codec: ptr::null(),
            variant,
            chan_in,
            chan_out,
            pts: 0,
        }
    }

    unsafe fn setup_encoder(&mut self) -> Result<(), Error> {
        if self.ctx.is_null() {
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
            if ret < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            // let downstream steps know about the encoder
            self.chan_out
                .send(PipelinePayload::EncoderInfo(self.variant.id(), ctx))?;

            self.ctx = ctx;
            self.codec = encoder;
        }
        Ok(())
    }

    unsafe fn process_frame(
        &mut self,
        frame: *mut AVFrame,
        in_tb: &AVRational,
    ) -> Result<(), Error> {
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
                    return Ok(());
                }
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            //set_encoded_pkt_timing(self.ctx, pkt, in_tb, &mut self.pts, &self.variant);
            av_packet_rescale_ts(pkt, *in_tb, self.variant.time_base());
            //dump_pkt_info(pkt);
            self.chan_out.send(PipelinePayload::AvPacket(
                pkt,
                AVPacketSource::Encoder(self.variant.id()),
            ))?;
        }

        Ok(())
    }
}

impl<TRecv> PipelineProcessor for VideoEncoder<TRecv>
where
    TRecv: Rx<PipelinePayload>,
{
    fn process(&mut self) -> Result<(), Error> {
        unsafe {
            self.setup_encoder()?;
        }
        while let Ok(pkg) = self.chan_in.try_recv_next() {
            match pkg {
                PipelinePayload::AvFrame(frm, ref src) => unsafe {
                    let (in_stream, idx) = match src {
                        AVFrameSource::Decoder(s) => (*s, (*(*s)).index as usize),
                        AVFrameSource::None(s) => (ptr::null_mut(), *s),
                        _ => {
                            return Err(Error::msg(format!("Cannot process frame from: {:?}", src)))
                        }
                    };
                    if self.variant.src_index == idx {
                        let tb = if in_stream.is_null() {
                            self.variant.time_base()
                        } else {
                            (*in_stream).time_base
                        };
                        self.process_frame(frm, &tb)?;
                    }
                },
                PipelinePayload::Flush => unsafe {
                    self.process_frame(ptr::null_mut(), &AVRational { num: 0, den: 1 })?;
                },
                _ => return Err(Error::msg("Payload not supported")),
            }
        }
        Ok(())
    }
}
