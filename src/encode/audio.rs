use std::ffi::CStr;
use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_audio_fifo_alloc, av_audio_fifo_free, av_audio_fifo_read, av_audio_fifo_size,
    av_audio_fifo_write, av_channel_layout_copy, av_frame_alloc, av_frame_free,
    av_get_sample_fmt_name, av_packet_alloc, av_packet_free, av_packet_rescale_ts,
    av_samples_alloc_array_and_samples, AVAudioFifo, AVCodec,
    avcodec_alloc_context3, avcodec_free_context, avcodec_open2, avcodec_receive_packet,
    avcodec_send_frame, AVCodecContext, AVERROR, AVFrame, AVRational, swr_alloc_set_opts2, swr_convert_frame,
    swr_free, swr_init, SwrContext,
};
use libc::EAGAIN;
use log::info;

use crate::pipeline::{AVFrameSource, AVPacketSource, PipelinePayload, PipelineProcessor};
use crate::return_ffmpeg_error;
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::{EncodedStream, StreamMapping};
use crate::variant::audio::AudioVariant;

pub struct AudioEncoder {
    variant: AudioVariant,
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    swr_ctx: *mut SwrContext,
    fifo: *mut AVAudioFifo,
    pts: i64,
}

unsafe impl Send for AudioEncoder {}

unsafe impl Sync for AudioEncoder {}

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        unsafe {
            swr_free(&mut self.swr_ctx);
            av_audio_fifo_free(self.fifo);
            avcodec_free_context(&mut self.ctx);
        }
    }
}

impl AudioEncoder {
    pub fn new(variant: AudioVariant) -> Self {
        Self {
            ctx: ptr::null_mut(),
            codec: ptr::null(),
            swr_ctx: ptr::null_mut(),
            fifo: ptr::null_mut(),
            variant,
            pts: 0,
        }
    }

    unsafe fn setup_encoder(
        &mut self,
        frame: *mut AVFrame,
    ) -> Result<Option<PipelinePayload>, Error> {
        if !self.ctx.is_null() {
            return Ok(None);
        }

        let encoder = self.variant.get_codec();
        if encoder.is_null() {
            return Err(Error::msg("Encoder not found"));
        }

        let ctx = avcodec_alloc_context3(encoder);
        if ctx.is_null() {
            return Err(Error::msg("Failed to allocate encoder context"));
        }

        self.variant.to_codec_context(ctx);

        // setup re-sampler if output format does not match input format
        if (*ctx).sample_fmt != transmute((*frame).format)
            || (*ctx).sample_rate != (*frame).sample_rate
            || (*ctx).ch_layout.nb_channels != (*frame).ch_layout.nb_channels
        {
            info!(
                "Setup audio resampler: {}.{}@{} -> {}.{}@{}",
                (*frame).ch_layout.nb_channels,
                CStr::from_ptr(av_get_sample_fmt_name(transmute((*frame).format)))
                    .to_str()?,
                (*frame).sample_rate,
                (*ctx).ch_layout.nb_channels,
                CStr::from_ptr(av_get_sample_fmt_name((*ctx).sample_fmt))
                    .to_str()?,
                (*ctx).sample_rate
            );

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
            return_ffmpeg_error!(ret);

            let ret = swr_init(swr_ctx);
            return_ffmpeg_error!(ret);

            self.swr_ctx = swr_ctx;

            let fifo = av_audio_fifo_alloc((*ctx).sample_fmt, (*ctx).ch_layout.nb_channels, 1);
            if fifo.is_null() {
                return Err(Error::msg("Failed to allocate audio FIFO"));
            }

            self.fifo = fifo;
        }

        let ret = avcodec_open2(ctx, encoder, ptr::null_mut());
        return_ffmpeg_error!(ret);

        self.ctx = ctx;
        self.codec = encoder;
        Ok(Some(PipelinePayload::EncoderInfo(self.variant.id(), ctx)))
    }

    /// Returns true if we should process audio frame from FIFO
    /// false if nothing to process this frame
    unsafe fn process_audio_frame(
        &mut self,
        frame: *mut AVFrame,
    ) -> Result<Option<*mut AVFrame>, Error> {
        if self.swr_ctx.is_null() {
            // no re-sampler, return input frame
            return Ok(Some(frame));
        }

        let mut out_frame = self.new_frame();
        let ret = swr_convert_frame(self.swr_ctx, out_frame, frame);
        if ret < 0 {
            av_frame_free(&mut out_frame);
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        let ret = av_audio_fifo_write(
            self.fifo,
            (*out_frame).extended_data as *const *mut libc::c_void,
            (*out_frame).nb_samples,
        );
        if ret < 0 {
            av_frame_free(&mut out_frame);
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }
        if ret != (*out_frame).nb_samples {
            av_frame_free(&mut out_frame);
            return Err(Error::msg(format!(
                "FIFO write {} != {}",
                ret,
                (*out_frame).nb_samples
            )));
        }

        //info!("Resampled {}->{} (wrote={})", in_samples, (*out_frame).nb_samples, ret);
        av_frame_free(&mut out_frame);

        let buff = av_audio_fifo_size(self.fifo);
        if buff < (*self.ctx).frame_size {
            Ok(None)
        } else {
            let out_frame = self.read_fifo_frame()?;
            Ok(Some(out_frame))
        }
    }

    unsafe fn read_fifo_frame(&mut self) -> Result<*mut AVFrame, Error> {
        let mut out_frame = self.new_frame();

        let ret = av_samples_alloc_array_and_samples(
            &mut (*out_frame).extended_data,
            ptr::null_mut(),
            (*out_frame).ch_layout.nb_channels,
            (*out_frame).nb_samples,
            transmute((*out_frame).format),
            0,
        );
        if ret < 0 {
            av_frame_free(&mut out_frame);
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        let ret = av_audio_fifo_read(
            self.fifo,
            (*out_frame).extended_data as *const *mut libc::c_void,
            (*out_frame).nb_samples,
        );
        if ret < 0 {
            av_frame_free(&mut out_frame);
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        assert_eq!(
            ret,
            (*out_frame).nb_samples,
            "Read wrong number of samples from FIFO"
        );
        Ok(out_frame)
    }

    unsafe fn new_frame(&self) -> *mut AVFrame {
        let out_frame = av_frame_alloc();
        (*out_frame).nb_samples = (*self.ctx).frame_size;
        av_channel_layout_copy(&mut (*out_frame).ch_layout, &(*self.ctx).ch_layout);
        (*out_frame).format = (*self.ctx).sample_fmt as libc::c_int;
        (*out_frame).sample_rate = (*self.ctx).sample_rate;
        out_frame
    }

    unsafe fn process_frame(
        &mut self,
        frame: *mut AVFrame,
        in_tb: &AVRational,
    ) -> Result<Vec<PipelinePayload>, Error> {
        let mut pkgs = Vec::new();
        if let Some(di) = self.setup_encoder(frame)? {
            pkgs.push(di);
        }
        let frame = self.process_audio_frame(frame)?;
        if frame.is_none() {
            return Ok(pkgs);
        }
        let mut frame = frame.unwrap();

        // examples do it like this
        (*frame).pts = self.pts;
        self.pts += (*frame).nb_samples as i64;

        let mut ret = avcodec_send_frame(self.ctx, frame);
        if ret < 0 && ret != AVERROR(EAGAIN) {
            av_frame_free(&mut frame);
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        while ret > 0 || ret == AVERROR(EAGAIN) {
            let mut pkt = av_packet_alloc();
            ret = avcodec_receive_packet(self.ctx, pkt);
            if ret < 0 {
                av_packet_free(&mut pkt);
                if ret == AVERROR(EAGAIN) {
                    break;
                }
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            //set_encoded_pkt_timing(self.ctx, pkt, in_tb, &mut self.pts, &self.variant);
            av_packet_rescale_ts(pkt, *in_tb, self.variant.time_base());
            pkgs.push(PipelinePayload::AvPacket(
                pkt,
                AVPacketSource::Encoder(self.variant.id()),
            ));
        }

        av_frame_free(&mut frame);
        Ok(pkgs)
    }
}

impl PipelineProcessor for AudioEncoder {
    fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error> {
        match pkg {
            PipelinePayload::AvFrame(frm, ref src) => unsafe {
                let in_stream = match src {
                    AVFrameSource::Decoder(s) => *s,
                    _ => return Err(Error::msg(format!("Cannot process frame from: {:?}", src))),
                };
                if self.variant.src_index() == (*in_stream).index as usize {
                    self.process_frame(frm, &(*in_stream).time_base)
                } else {
                    // stream didnt match, skipping
                    Ok(vec![])
                }
            },
            PipelinePayload::Flush => unsafe {
                Ok(self.process_frame(ptr::null_mut(), &AVRational { num: 0, den: 1 })?)
            },
            _ => Err(Error::msg("Payload not supported")),
        }
    }
}
