use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_buffer_ref, av_opt_set, av_packet_alloc, av_packet_free, av_packet_rescale_ts,
    AVBufferRef, AVCodec, avcodec_alloc_context3, avcodec_find_encoder,
    avcodec_open2, avcodec_receive_packet, avcodec_send_frame, AVCodecContext, AVERROR, AVFrame, AVRational,
};
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
use libc::EAGAIN;
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::{get_ffmpeg_error_msg, id_ref_to_uuid, video_variant_id_ref};
use crate::variant::VideoVariant;

pub struct VideoEncoder<T> {
    variant: VideoVariant,
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    chan_in: T,
    chan_out: UnboundedSender<PipelinePayload>,
    var_id_ref: *mut AVBufferRef,
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
        let id_ref = video_variant_id_ref(&variant);
        Self {
            ctx: ptr::null_mut(),
            codec: ptr::null(),
            variant,
            chan_in,
            chan_out,
            var_id_ref: id_ref,
        }
    }

    unsafe fn setup_encoder(&mut self, frame: *mut AVFrame) -> Result<(), Error> {
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

            (*ctx).time_base = self.variant.time_base();
            (*ctx).bit_rate = self.variant.bitrate as i64;
            (*ctx).width = (*frame).width;
            (*ctx).height = (*frame).height;
            (*ctx).level = self.variant.level as libc::c_int;
            (*ctx).profile = self.variant.profile as libc::c_int;
            (*ctx).framerate = AVRational {
                num: 1,
                den: self.variant.fps as libc::c_int,
            };

            let key_frames = self.variant.fps * self.variant.keyframe_interval;
            (*ctx).gop_size = key_frames as libc::c_int;
            (*ctx).max_b_frames = 1;
            (*ctx).pix_fmt = AV_PIX_FMT_YUV420P;
            (*ctx).colorspace = AVCOL_SPC_BT709;
            av_opt_set(
                (*ctx).priv_data,
                "preset\0".as_ptr() as *const libc::c_char,
                "fast\0".as_ptr() as *const libc::c_char,
                0,
            );

            let ret = avcodec_open2(ctx, encoder, ptr::null_mut());
            if ret < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            self.ctx = ctx;
            self.codec = encoder;
        }
        Ok(())
    }

    unsafe fn process_frame(&mut self, frame: *mut AVFrame) -> Result<(), Error> {
        let var_id = id_ref_to_uuid((*frame).opaque_ref)?;
        assert_eq!(var_id, self.variant.id);

        self.setup_encoder(frame)?;

        let mut ret = avcodec_send_frame(self.ctx, frame);
        if ret < 0 && ret != AVERROR(EAGAIN) {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        while ret > 0 || ret == AVERROR(EAGAIN) {
            let mut pkt = av_packet_alloc();
            ret = avcodec_receive_packet(self.ctx, pkt);
            if ret < 0 {
                av_packet_free(&mut pkt);
                if ret == AVERROR(EAGAIN) {
                    return Ok(());
                }
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            (*pkt).time_base = (*self.ctx).time_base;
            (*pkt).duration = (*frame).duration;
            av_packet_rescale_ts(pkt, (*frame).time_base, (*self.ctx).time_base);
            (*pkt).opaque = self.ctx as *mut libc::c_void;
            (*pkt).opaque_ref = av_buffer_ref(self.var_id_ref);
            self.chan_out
                .send(PipelinePayload::AvPacket("Encoder packet".to_owned(), pkt))?;
        }

        Ok(())
    }
}

impl<TRecv> PipelineProcessor for VideoEncoder<TRecv>
    where
        TRecv: Rx<PipelinePayload>,
{
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv_next() {
            match pkg {
                PipelinePayload::AvFrame(_, frm, idx) => unsafe {
                    if self.variant.src_index == idx {
                        self.process_frame(frm)?;
                    }
                },
                _ => return Err(Error::msg("Payload not supported")),
            }
        }
        Ok(())
    }
}
