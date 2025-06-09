use crate::overseer::IngressStream;
use anyhow::{bail, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_RGB;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_RGBA;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_channel_layout_default, av_frame_alloc, av_frame_free, av_frame_get_buffer, AVFrame,
    AVPixelFormat, AVRational,
};
use ffmpeg_rs_raw::Scaler;
use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
use fontdue::Font;
use std::mem::transmute;
use std::time::{Duration, Instant};
use std::{ptr, slice};

/// Frame generator
pub struct FrameGenerator {
    fps: f32,
    width: u16,
    height: u16,
    video_sample_fmt: AVPixelFormat,

    audio_sample_rate: u32,
    audio_frame_size: i32,
    audio_channels: u8,

    frame_idx: u64,
    audio_samples: u64,

    // internal
    next_frame: *mut AVFrame,
    scaler: Scaler,
    font: Font,
    start: Instant,
}

impl Drop for FrameGenerator {
    fn drop(&mut self) {
        unsafe {
            if !self.next_frame.is_null() {
                av_frame_free(&mut self.next_frame);
                self.next_frame = std::ptr::null_mut();
            }
        }
    }
}

impl FrameGenerator {
    pub fn new(
        fps: f32,
        width: u16,
        height: u16,
        pix_fmt: AVPixelFormat,
        sample_rate: u32,
        frame_size: i32,
        channels: u8,
    ) -> Result<Self> {
        let font = include_bytes!("../SourceCodePro-Regular.ttf") as &[u8];
        let font = Font::from_bytes(font, Default::default()).unwrap();

        Ok(Self {
            fps,
            width,
            height,
            video_sample_fmt: pix_fmt,
            audio_sample_rate: sample_rate,
            audio_frame_size: frame_size,
            audio_channels: channels,
            frame_idx: 0,
            audio_samples: 0,
            font,
            start: Instant::now(),
            scaler: Scaler::default(),
            next_frame: ptr::null_mut(),
        })
    }

    pub fn from_stream(
        video_stream: &IngressStream,
        audio_stream: Option<&IngressStream>,
    ) -> Result<Self> {
        Ok(Self::new(
            video_stream.fps,
            video_stream.width as _,
            video_stream.height as _,
            unsafe { transmute(video_stream.format as i32) },
            audio_stream.map(|i| i.sample_rate as _).unwrap_or(0),
            if audio_stream.is_none() { 0 } else { 1024 },
            audio_stream.map(|i| i.channels as _).unwrap_or(0),
        )?)
    }

    pub fn frame_no(&self) -> u64 {
        self.frame_idx
    }

    /// Create a new frame for composing text / images
    pub fn begin(&mut self) -> Result<()> {
        if self.next_frame.is_null() {
            unsafe {
                let mut src_frame = av_frame_alloc();
                if src_frame.is_null() {
                    bail!("Failed to allocate placeholder video frame");
                }

                (*src_frame).width = self.width as _;
                (*src_frame).height = self.height as _;
                (*src_frame).pict_type = AV_PICTURE_TYPE_NONE;
                (*src_frame).key_frame = 1;
                (*src_frame).colorspace = AVCOL_SPC_RGB;
                //internally always use RGBA, we convert frame to target pixel format at the end
                (*src_frame).format = AV_PIX_FMT_RGBA as _;
                (*src_frame).pts = self.frame_idx as _;
                (*src_frame).duration = 1;
                (*src_frame).time_base = AVRational {
                    num: 1,
                    den: self.fps as i32,
                };
                if av_frame_get_buffer(src_frame, 0) < 0 {
                    av_frame_free(&mut src_frame);
                    bail!("Failed to get frame buffer");
                }
                self.next_frame = src_frame;
            }
        }
        Ok(())
    }

    /// Write some text into the next frame
    pub fn write_text(&mut self, msg: &str, size: f32, x: f32, y: f32) -> Result<()> {
        if self.next_frame.is_null() {
            bail!("Must call begin() before writing text")
        }
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.append(&[&self.font], &TextStyle::new(msg, size, 0));

        self.write_layout(layout, x, y)?;
        Ok(())
    }

    /// Write text layout into frame
    fn write_layout(&mut self, layout: Layout, x: f32, y: f32) -> Result<()> {
        for g in layout.glyphs() {
            let (metrics, bitmap) = self.font.rasterize_config_subpixel(g.key);
            for y1 in 0..metrics.height {
                for x1 in 0..metrics.width {
                    let dst_x = x as usize + x1 + g.x as usize;
                    let dst_y = y as usize + y1 + g.y as usize;
                    let offset_src = (x1 + y1 * metrics.width) * 3;
                    unsafe {
                        let offset_dst =
                            4 * dst_x + dst_y * (*self.next_frame).linesize[0] as usize;
                        let pixel_dst = (*self.next_frame).data[0].add(offset_dst);
                        *pixel_dst.offset(0) = bitmap[offset_src];
                        *pixel_dst.offset(1) = bitmap[offset_src + 1];
                        *pixel_dst.offset(2) = bitmap[offset_src + 2];
                    }
                }
            }
        }
        Ok(())
    }

    /// Copy data directly into the frame buffer (must be RGBA data)
    pub unsafe fn copy_frame_data(&mut self, data: &[u8]) -> Result<()> {
        if self.next_frame.is_null() {
            bail!("Must call begin() before writing frame data")
        }
        let buf = slice::from_raw_parts_mut(
            (*self.next_frame).data[0],
            (self.width as usize * self.height as usize * 4) as usize,
        );
        if buf.len() < data.len() {
            bail!("Frame buffer is too small");
        }
        buf.copy_from_slice(data);
        Ok(())
    }

    /// Generate audio to stay synchronized with video frames
    unsafe fn generate_audio_frame(&mut self) -> Result<*mut AVFrame> {
        const FREQUENCY: f32 = 440.0; // A4 note

        // audio is disabled if sample rate is 0
        if self.audio_sample_rate == 0 {
            return Ok(ptr::null_mut());
        }

        // Calculate how many audio samples we need to cover the next video frame
        let samples_per_frame = (self.audio_sample_rate as f32 / self.fps) as u64;
        let next_frame_needs_samples = (self.frame_idx + 1) * samples_per_frame;

        // Generate audio if we don't have enough to cover the next video frame
        if self.audio_samples < next_frame_needs_samples {
            let audio_frame = av_frame_alloc();
            (*audio_frame).format = AV_SAMPLE_FMT_FLTP as _;
            (*audio_frame).nb_samples = self.audio_frame_size as _;
            (*audio_frame).duration = self.audio_frame_size as _;
            (*audio_frame).sample_rate = self.audio_sample_rate as _;
            (*audio_frame).pts = self.audio_samples as _;
            (*audio_frame).time_base = AVRational {
                num: 1,
                den: self.audio_sample_rate as _,
            };
            av_channel_layout_default(&mut (*audio_frame).ch_layout, self.audio_channels as _);
            av_frame_get_buffer(audio_frame, 0);

            // Generate sine wave samples
            let data = (*audio_frame).data[0] as *mut f32;
            for i in 0..self.audio_frame_size {
                let sample_time =
                    (self.audio_samples + i as u64) as f32 / self.audio_sample_rate as f32;
                let sample_value =
                    (2.0 * std::f32::consts::PI * FREQUENCY * sample_time).sin() * 0.5;
                *data.add(i as _) = sample_value;
            }

            self.audio_samples += self.audio_frame_size as u64;
            return Ok(audio_frame);
        }

        Ok(ptr::null_mut())
    }

    /// Return the next frame for encoding (blocking)
    pub unsafe fn next(&mut self) -> Result<*mut AVFrame> {
        // set start time to now if this is the first call to next()
        if self.frame_idx == 0 {
            self.start = Instant::now();
        }

        // try to get audio frames before video frames (non-blocking)
        let audio_frame = self.generate_audio_frame()?;
        if !audio_frame.is_null() {
            return Ok(audio_frame);
        }

        // auto-init frame
        if self.next_frame.is_null() {
            self.begin()?;
        }

        let stream_time = Duration::from_secs_f64(self.frame_idx as f64 / self.fps as f64);
        let real_time = Instant::now().duration_since(self.start);
        let wait_time = if stream_time > real_time {
            stream_time - real_time
        } else {
            Duration::new(0, 0)
        };
        if !wait_time.is_zero() && wait_time.as_secs_f32() > 1f32 / self.fps {
            std::thread::sleep(wait_time);
        }

        // convert to output pixel format, or just return internal frame if it matches output
        if self.video_sample_fmt != transmute((*self.next_frame).format) {
            let out_frame = self.scaler.process_frame(
                self.next_frame,
                self.width,
                self.height,
                self.video_sample_fmt,
            )?;
            av_frame_free(&mut self.next_frame);
            self.next_frame = ptr::null_mut();
            self.frame_idx += 1;
            Ok(out_frame)
        } else {
            let ret = self.next_frame;
            self.next_frame = ptr::null_mut();
            self.frame_idx += 1;
            Ok(ret)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;

    #[test]
    fn test_frame_timing_synchronization() {
        unsafe {
            let fps = 30.0;
            let sample_rate = 44100;
            let frame_size = 1024;
            let channels = 2;

            let mut gen = FrameGenerator::new(
                fps,
                1280,
                720,
                AV_PIX_FMT_YUV420P,
                sample_rate,
                frame_size,
                channels,
            )
            .unwrap();

            let samples_per_frame = sample_rate as f64 / fps as f64; // Expected: 1470 samples per frame
            println!("Expected samples per video frame: {:.2}", samples_per_frame);

            let mut audio_frames = 0;
            let mut video_frames = 0;
            let mut total_audio_samples = 0;

            // Generate frames for 2 seconds (60 video frames at 30fps)
            for i in 0..120 {
                let mut frame = gen.next().unwrap();

                if (*frame).sample_rate > 0 {
                    // Audio frame
                    audio_frames += 1;
                    total_audio_samples += (*frame).nb_samples as u64;
                    println!(
                        "Frame {}: AUDIO - PTS: {}, samples: {}, total_samples: {}",
                        i,
                        (*frame).pts,
                        (*frame).nb_samples,
                        total_audio_samples
                    );
                } else {
                    // Video frame
                    video_frames += 1;
                    let expected_audio_samples = (video_frames as f64 * samples_per_frame) as u64;
                    let audio_deficit = if total_audio_samples >= expected_audio_samples {
                        0
                    } else {
                        expected_audio_samples - total_audio_samples
                    };

                    println!("Frame {}: VIDEO - PTS: {}, frame_idx: {}, expected_audio: {}, actual_audio: {}, deficit: {}", 
                             i, (*frame).pts, video_frames, expected_audio_samples, total_audio_samples, audio_deficit);

                    // Verify we have enough audio for this video frame
                    assert!(
                        total_audio_samples >= expected_audio_samples,
                        "Video frame {} needs {} audio samples but only have {}",
                        video_frames,
                        expected_audio_samples,
                        total_audio_samples
                    );
                }

                av_frame_free(&mut frame);
            }

            println!("\nSummary:");
            println!("Video frames: {}", video_frames);
            println!("Audio frames: {}", audio_frames);
            println!("Total audio samples: {}", total_audio_samples);
            println!(
                "Expected audio samples for {} video frames: {:.2}",
                video_frames,
                video_frames as f64 * samples_per_frame
            );

            // Verify the ratio is correct
            let expected_total_audio = video_frames as f64 * samples_per_frame;
            let sample_accuracy = (total_audio_samples as f64 - expected_total_audio).abs();
            println!("Sample accuracy (difference): {:.2}", sample_accuracy);

            // Allow for some tolerance due to frame size constraints
            assert!(
                sample_accuracy < frame_size as f64,
                "Audio sample count too far from expected: got {}, expected {:.2}, diff {:.2}",
                total_audio_samples,
                expected_total_audio,
                sample_accuracy
            );
        }
    }

    #[test]
    fn test_pts_progression() {
        unsafe {
            let fps = 30.0;
            let sample_rate = 44100;

            let mut gen =
                FrameGenerator::new(fps, 1280, 720, AV_PIX_FMT_YUV420P, sample_rate, 1024, 2)
                    .unwrap();

            let mut last_audio_pts = -1i64;
            let mut last_video_pts = -1i64;
            let mut audio_pts_gaps = Vec::new();
            let mut video_pts_gaps = Vec::new();

            // Generate 60 frames to test PTS progression
            for _ in 0..60 {
                let mut frame = gen.next().unwrap();

                if (*frame).sample_rate > 0 {
                    // Audio frame - check PTS progression
                    if last_audio_pts >= 0 {
                        let gap = (*frame).pts - last_audio_pts;
                        audio_pts_gaps.push(gap);
                        println!("Audio PTS gap: {}", gap);
                    }
                    last_audio_pts = (*frame).pts;
                } else {
                    // Video frame - check PTS progression
                    if last_video_pts >= 0 {
                        let gap = (*frame).pts - last_video_pts;
                        video_pts_gaps.push(gap);
                        println!("Video PTS gap: {}", gap);
                    }
                    last_video_pts = (*frame).pts;
                }

                av_frame_free(&mut frame);
            }

            // Verify audio PTS gaps are consistent (should be 1024 samples)
            for gap in &audio_pts_gaps {
                assert_eq!(
                    *gap, 1024,
                    "Audio PTS should increment by frame_size (1024)"
                );
            }

            // Verify video PTS gaps are consistent (should be 1 frame)
            for gap in &video_pts_gaps {
                assert_eq!(*gap, 1, "Video PTS should increment by 1 frame");
            }

            println!("PTS progression test passed - all gaps are consistent");
        }
    }
}
