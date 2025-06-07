use anyhow::{bail, Result};
use crate::variant::video::VideoVariant;
use crate::variant::audio::AudioVariant;
use crate::overseer::{IngressStream, IngressStreamType};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_alloc, av_frame_get_buffer, av_frame_free, av_get_sample_fmt, AVFrame, 
    AVPixelFormat, AVSampleFormat
};
use std::ffi::CString;

/// Placeholder frame generator for idle mode when stream disconnects
pub struct PlaceholderGenerator;

impl PlaceholderGenerator {
    /// Generate a placeholder video frame based on ingress stream info
    pub unsafe fn generate_video_frame_from_stream(
        stream: &IngressStream,
        stream_time_base: (i32, i32),
        frame_index: u64
    ) -> Result<*mut AVFrame> {
        let frame = av_frame_alloc();
        if frame.is_null() {
            bail!("Failed to allocate placeholder video frame");
        }

        (*frame).format = AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
        (*frame).width = stream.width as i32;
        (*frame).height = stream.height as i32;
        (*frame).time_base.num = stream_time_base.0;
        (*frame).time_base.den = stream_time_base.1;
        
        // Set PTS based on frame rate and total frame index
        let fps = if stream.fps > 0.0 { stream.fps } else { 30.0 };
        let time_base_f64 = stream_time_base.0 as f64 / stream_time_base.1 as f64;
        (*frame).pts = (frame_index as f64 / fps / time_base_f64) as i64;

        if av_frame_get_buffer(frame, 0) < 0 {
            av_frame_free(&mut frame);
            bail!("Failed to allocate buffer for placeholder video frame");
        }

        // Fill with black (Y=16, U=V=128 for limited range YUV420P)
        let y_size = ((*frame).width * (*frame).height) as usize;
        let uv_size = y_size / 4;
        
        if !(*frame).data[0].is_null() {
            std::ptr::write_bytes((*frame).data[0], 16, y_size);
        }
        if !(*frame).data[1].is_null() {
            std::ptr::write_bytes((*frame).data[1], 128, uv_size);
        }
        if !(*frame).data[2].is_null() {
            std::ptr::write_bytes((*frame).data[2], 128, uv_size);
        }

        Ok(frame)
    }

    /// Generate a placeholder audio frame based on ingress stream info  
    pub unsafe fn generate_audio_frame_from_stream(
        stream: &IngressStream,
        stream_time_base: (i32, i32),
        frame_index: u64,
        sample_fmt: &str,
        channels: u32
    ) -> Result<*mut AVFrame> {
        let frame = av_frame_alloc();
        if frame.is_null() {
            bail!("Failed to allocate placeholder audio frame");
        }

        // Use the provided sample format
        let sample_fmt_cstr = CString::new(sample_fmt)
            .map_err(|_| anyhow::anyhow!("Invalid sample format string"))?;
        let sample_fmt_int = av_get_sample_fmt(sample_fmt_cstr.as_ptr());
        (*frame).format = sample_fmt_int;
        (*frame).channels = channels as i32;
        (*frame).sample_rate = stream.sample_rate as i32;
        (*frame).nb_samples = 1024; // Standard audio frame size
        (*frame).time_base.num = stream_time_base.0;
        (*frame).time_base.den = stream_time_base.1;
        
        // Set PTS based on sample rate and frame index
        let samples_per_second = stream.sample_rate as f64;
        let time_base_f64 = stream_time_base.0 as f64 / stream_time_base.1 as f64;
        (*frame).pts = ((frame_index * 1024) as f64 / samples_per_second / time_base_f64) as i64;

        if av_frame_get_buffer(frame, 0) < 0 {
            av_frame_free(&mut frame);
            bail!("Failed to allocate buffer for placeholder audio frame");
        }

        // Fill with silence (zeros)
        for i in 0..8 {
            if !(*frame).data[i].is_null() && (*frame).linesize[i] > 0 {
                std::ptr::write_bytes((*frame).data[i], 0, (*frame).linesize[i] as usize);
            }
        }

        Ok(frame)
    }

    /// Generate a placeholder black video frame
    pub unsafe fn generate_video_frame(
        variant: &VideoVariant, 
        stream_time_base: (i32, i32),
        frame_index: u64
    ) -> Result<*mut AVFrame> {
        let frame = av_frame_alloc();
        if frame.is_null() {
            bail!("Failed to allocate placeholder video frame");
        }

        (*frame).format = AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
        (*frame).width = variant.width as i32;
        (*frame).height = variant.height as i32;
        (*frame).time_base.num = stream_time_base.0;
        (*frame).time_base.den = stream_time_base.1;
        
        // Set PTS based on frame rate and total frame index
        let fps = if variant.fps > 0.0 { variant.fps } else { 30.0 };
        let time_base_f64 = stream_time_base.0 as f64 / stream_time_base.1 as f64;
        (*frame).pts = (frame_index as f64 / fps / time_base_f64) as i64;

        if av_frame_get_buffer(frame, 0) < 0 {
            av_frame_free(&mut frame);
            bail!("Failed to allocate buffer for placeholder video frame");
        }

        // Fill with black (Y=16, U=V=128 for limited range YUV420P)
        let y_size = ((*frame).width * (*frame).height) as usize;
        let uv_size = y_size / 4;
        
        if !(*frame).data[0].is_null() {
            std::ptr::write_bytes((*frame).data[0], 16, y_size);
        }
        if !(*frame).data[1].is_null() {
            std::ptr::write_bytes((*frame).data[1], 128, uv_size);
        }
        if !(*frame).data[2].is_null() {
            std::ptr::write_bytes((*frame).data[2], 128, uv_size);
        }

        Ok(frame)
    }

    /// Generate a placeholder silent audio frame
    pub unsafe fn generate_audio_frame(
        variant: &AudioVariant, 
        stream_time_base: (i32, i32),
        frame_index: u64
    ) -> Result<*mut AVFrame> {
        let frame = av_frame_alloc();
        if frame.is_null() {
            bail!("Failed to allocate placeholder audio frame");
        }

        // Use the sample format from the variant configuration
        let sample_fmt_cstr = CString::new(variant.sample_fmt.as_str())
            .map_err(|_| anyhow::anyhow!("Invalid sample format string"))?;
        let sample_fmt_int = av_get_sample_fmt(sample_fmt_cstr.as_ptr());
        (*frame).format = sample_fmt_int;
        (*frame).channels = variant.channels as i32;
        (*frame).sample_rate = variant.sample_rate as i32;
        (*frame).nb_samples = 1024; // Standard audio frame size
        (*frame).time_base.num = stream_time_base.0;
        (*frame).time_base.den = stream_time_base.1;
        
        // Set PTS based on sample rate and frame index
        let samples_per_second = variant.sample_rate as f64;
        let time_base_f64 = stream_time_base.0 as f64 / stream_time_base.1 as f64;
        (*frame).pts = ((frame_index * 1024) as f64 / samples_per_second / time_base_f64) as i64;

        if av_frame_get_buffer(frame, 0) < 0 {
            av_frame_free(&mut frame);
            bail!("Failed to allocate buffer for placeholder audio frame");
        }

        // Fill with silence (zeros)
        for i in 0..8 {
            if !(*frame).data[i].is_null() && (*frame).linesize[i] > 0 {
                std::ptr::write_bytes((*frame).data[i], 0, (*frame).linesize[i] as usize);
            }
        }

        Ok(frame)
    }
}