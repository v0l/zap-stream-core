use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_buffer_ref, AV_CH_LAYOUT_STEREO, AV_CODEC_FLAG_GLOBAL_HEADER, av_get_sample_fmt, av_opt_set,
    av_packet_alloc, av_packet_free, AVBufferRef, AVChannelLayout,
    AVChannelLayout__bindgen_ty_1, AVCodec, avcodec_alloc_context3, avcodec_find_encoder, avcodec_open2,
    avcodec_receive_packet, avcodec_send_frame, AVCodecContext, AVERROR, AVFrame, AVRational,
    AVStream,
};
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
use libc::EAGAIN;
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::PipelinePayload;
use crate::utils::{get_ffmpeg_error_msg, variant_id_ref};
use crate::variant::VariantStream;

pub struct Encoder<T> {
    variant: VariantStream,
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    chan_in: T,
    chan_out: UnboundedSender<PipelinePayload>,
    var_id_ref: *mut AVBufferRef,
}

unsafe impl<T> Send for Encoder<T> {}

unsafe impl<T> Sync for Encoder<T> {}

impl<TRecv> Encoder<TRecv>
    where
        TRecv: Rx<PipelinePayload>,
{
    pub fn new(
        chan_in: TRecv,
        chan_out: UnboundedSender<PipelinePayload>,
        variant: VariantStream,
    ) -> Self {
        let id_ref = variant_id_ref(&variant).unwrap();
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
        if self.ctx == ptr::null_mut() {
            let codec = match &self.variant {
                VariantStream::Video(vv) => vv.codec,
                VariantStream::Audio(va) => va.codec,
                _ => return Err(Error::msg("Not supported")),
            };
            let encoder = avcodec_find_encoder(transmute(codec as i32));
            if encoder == ptr::null_mut() {
                return Err(Error::msg("Encoder not found"));
            }

            let ctx = avcodec_alloc_context3(encoder);
            if ctx == ptr::null_mut() {
                return Err(Error::msg("Failed to allocate encoder context"));
            }

            match &self.variant {
                VariantStream::Video(vv) => {
                    (*ctx).bit_rate = vv.bitrate as i64;
                    (*ctx).width = (*frame).width;
                    (*ctx).height = (*frame).height;
                    (*ctx).time_base = AVRational {
                        num: 1,
                        den: vv.fps as libc::c_int,
                    };

                    (*ctx).gop_size = (vv.fps * vv.keyframe_interval) as libc::c_int;
                    (*ctx).max_b_frames = 1;
                    (*ctx).pix_fmt = AV_PIX_FMT_YUV420P;
                    av_opt_set(
                        (*ctx).priv_data,
                        "preset\0".as_ptr() as *const libc::c_char,
                        "fast\0".as_ptr() as *const libc::c_char,
                        0,
                    );
                }
                VariantStream::Audio(va) => {
                    (*ctx).sample_fmt = av_get_sample_fmt(
                        format!("{}\0", va.sample_fmt).as_ptr() as *const libc::c_char
                    );
                    (*ctx).bit_rate = va.bitrate as i64;
                    (*ctx).sample_rate = va.sample_rate as libc::c_int;
                    (*ctx).ch_layout = AVChannelLayout {
                        order: AV_CHANNEL_ORDER_NATIVE,
                        nb_channels: 2,
                        u: AVChannelLayout__bindgen_ty_1 {
                            mask: AV_CH_LAYOUT_STEREO,
                        },
                        opaque: ptr::null_mut(),
                    };
                    (*ctx).time_base = AVRational {
                        num: 1,
                        den: va.sample_rate as libc::c_int,
                    };
                }
                _ => {
                    // nothing
                }
            };

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
        let stream = (*frame).opaque as *mut AVStream;
        if (*stream).index as usize != self.variant.src_index() {
            return Ok(());
        }
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

            (*pkt).duration = (*frame).duration;
            (*pkt).time_base = (*frame).time_base;
            (*pkt).opaque = stream as *mut libc::c_void;
            (*pkt).opaque_ref = av_buffer_ref(self.var_id_ref);
            self.chan_out.send(PipelinePayload::AvPacket(pkt))?;
        }

        Ok(())
    }

    pub fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv_next() {
            match pkg {
                PipelinePayload::AvFrame(frm) => unsafe {
                    self.process_frame(frm)?;
                },
                _ => return Err(Error::msg("Payload not supported")),
            }
        }
        Ok(())
    }
}
