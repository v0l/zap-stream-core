use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_audio_fifo_alloc, av_audio_fifo_free, av_audio_fifo_read, av_audio_fifo_realloc,
    av_audio_fifo_size, av_audio_fifo_write, av_buffer_ref, av_buffer_unref,
    AV_CH_LAYOUT_STEREO, av_channel_layout_copy, av_frame_alloc, av_frame_free, av_frame_get_buffer,
    av_freep, av_get_sample_fmt, av_packet_alloc, av_packet_free,
    av_packet_rescale_ts, av_samples_alloc_array_and_samples, AVAudioFifo,
    AVBufferRef, AVChannelLayout, AVChannelLayout__bindgen_ty_1, AVCodec,
    avcodec_alloc_context3, avcodec_find_encoder, avcodec_free_context, avcodec_open2, avcodec_receive_packet, avcodec_send_frame,
    AVCodecContext, AVERROR, AVFrame, swr_alloc_set_opts2, swr_convert, swr_free,
    swr_init, SwrContext,
};
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use libc::EAGAIN;
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::{audio_variant_id_ref, get_ffmpeg_error_msg, id_ref_to_uuid};
use crate::variant::AudioVariant;

pub struct AudioEncoder<T> {
    variant: AudioVariant,
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    fifo: *mut AVAudioFifo,
    swr_ctx: *mut SwrContext,
    chan_in: T,
    chan_out: UnboundedSender<PipelinePayload>,
    var_id_ref: *mut AVBufferRef,
}

unsafe impl<T> Send for AudioEncoder<T> {}

unsafe impl<T> Sync for AudioEncoder<T> {}

impl<T> Drop for AudioEncoder<T> {
    fn drop(&mut self) {
        unsafe {
            swr_free(&mut self.swr_ctx);
            av_audio_fifo_free(self.fifo);
            avcodec_free_context(&mut self.ctx);
            av_buffer_unref(&mut self.var_id_ref);
        }
    }
}

impl<TRecv> AudioEncoder<TRecv>
    where
        TRecv: Rx<PipelinePayload>,
{
    pub fn new(
        chan_in: TRecv,
        chan_out: UnboundedSender<PipelinePayload>,
        variant: AudioVariant,
    ) -> Self {
        let id_ref = audio_variant_id_ref(&variant);
        Self {
            ctx: ptr::null_mut(),
            codec: ptr::null(),
            fifo: ptr::null_mut(),
            swr_ctx: ptr::null_mut(),
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
            (*ctx).sample_fmt = av_get_sample_fmt(
                format!("{}\0", self.variant.sample_fmt).as_ptr() as *const libc::c_char,
            );
            (*ctx).bit_rate = self.variant.bitrate as i64;
            (*ctx).sample_rate = self.variant.sample_rate as libc::c_int;
            (*ctx).ch_layout = AVChannelLayout {
                order: AV_CHANNEL_ORDER_NATIVE,
                nb_channels: 2,
                u: AVChannelLayout__bindgen_ty_1 {
                    mask: AV_CH_LAYOUT_STEREO,
                },
                opaque: ptr::null_mut(),
            };

            // setup audio FIFO
            let fifo = av_audio_fifo_alloc((*ctx).sample_fmt, 2, 1);
            if fifo.is_null() {
                return Err(Error::msg("Failed to allocate audio FiFO buffer"));
            }

            let mut swr_ctx = ptr::null_mut();
            let ret = swr_alloc_set_opts2(
                &mut swr_ctx,
                &(*ctx).ch_layout,
                (*ctx).sample_fmt,
                (*ctx).sample_rate,
                &(*frame).ch_layout,
                transmute((*frame).format),
                (*frame).sample_rate,
                0,
                ptr::null_mut(),
            );
            if ret < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            let ret = swr_init(swr_ctx);
            if ret < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            let ret = avcodec_open2(ctx, encoder, ptr::null_mut());
            if ret < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            self.ctx = ctx;
            self.codec = encoder;
            self.swr_ctx = swr_ctx;
            self.fifo = fifo;
        }
        Ok(())
    }

    /// Returns true if we should process audio frame from FIFO
    /// false if nothing to process this frame
    unsafe fn process_audio_frame(&mut self, frame: *mut AVFrame) -> Result<bool, Error> {
        let in_samples = (*frame).nb_samples;
        let mut dst_samples: *mut *mut u8 = ptr::null_mut();
        let ret = av_samples_alloc_array_and_samples(
            &mut dst_samples,
            ptr::null_mut(),
            2,
            in_samples,
            (*self.ctx).sample_fmt,
            0,
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        // resample audio
        let ret = swr_convert(
            self.swr_ctx,
            dst_samples,
            in_samples,
            (*frame).extended_data as *const *const u8,
            in_samples,
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        // push resampled audio into fifo
        let ret = av_audio_fifo_realloc(self.fifo, av_audio_fifo_size(self.fifo) + in_samples);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }
        if av_audio_fifo_write(
            self.fifo,
            dst_samples as *const *mut libc::c_void,
            in_samples,
        ) < in_samples
        {
            return Err(Error::msg("Failed to write samples to FIFO"));
        }

        if !dst_samples.is_null() {
            av_freep(dst_samples.add(0) as *mut libc::c_void);
        }

        let buffered = av_audio_fifo_size(self.fifo);
        Ok(buffered >= (*self.ctx).frame_size)
    }

    unsafe fn get_fifo_frame(&mut self) -> Result<*mut AVFrame, Error> {
        let mut frame = av_frame_alloc();
        let frame_size = (*self.ctx).frame_size.min(av_audio_fifo_size(self.fifo));
        (*frame).nb_samples = frame_size;
        av_channel_layout_copy(&mut (*frame).ch_layout, &(*self.ctx).ch_layout);
        (*frame).format = (*self.ctx).sample_fmt as libc::c_int;
        (*frame).sample_rate = (*self.ctx).sample_rate;

        let ret = av_frame_get_buffer(frame, 0);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        let ret = av_audio_fifo_read(
            self.fifo,
            ptr::addr_of_mut!((*frame).data) as *const *mut libc::c_void,
            frame_size,
        );
        if ret < frame_size {
            av_frame_free(&mut frame);
            return Err(Error::msg("Failed to read frame from FIFO"));
        }

        Ok(frame)
    }

    unsafe fn process_frame(&mut self, frame: *mut AVFrame) -> Result<(), Error> {
        let var_id = id_ref_to_uuid((*frame).opaque_ref)?;
        assert_eq!(var_id, self.variant.id);

        self.setup_encoder(frame)?;

        if !self.process_audio_frame(frame)? {
            return Ok(());
        }

        // read audio from FIFO
        let frame = self.get_fifo_frame()?;
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

impl<TRecv> PipelineProcessor for AudioEncoder<TRecv>
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
