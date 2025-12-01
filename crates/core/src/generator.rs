use crate::overseer::IngressStream;
use anyhow::{Result, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_RGB;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_RGBA;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AVPixelFormat, AVRational, AVStream, av_channel_layout_default, av_frame_alloc, av_frame_free,
    av_frame_get_buffer, av_q2d, av_rescale_q,
};
use ffmpeg_rs_raw::{AvFrameRef, Scaler};
use fontdue::Font;
use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
use std::mem::transmute;
use std::ops::Sub;
use std::slice;
use std::time::{Duration, Instant};

/// Frame generator
pub struct FrameGenerator {
    fps: f32,
    width: u16,
    height: u16,
    video_sample_fmt: AVPixelFormat,
    realtime: bool,

    audio_sample_rate: u32,
    audio_frame_size: i32,
    audio_channels: u8,

    video_pts: i64,
    audio_pts: i64,

    // Timebases for frame generation
    video_timebase: AVRational,
    audio_timebase: AVRational,

    // internal
    next_frame: Option<AvFrameRef>,
    scaler: Scaler,
    font: Font,
    start: Instant,
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
        video_timebase: AVRational,
        audio_timebase: AVRational,
    ) -> Result<Self> {
        let font = include_bytes!("../SourceCodePro-Regular.ttf") as &[u8];
        let font = Font::from_bytes(font, Default::default()).unwrap();

        Ok(Self {
            fps,
            width,
            height,
            realtime: true,
            video_sample_fmt: pix_fmt,
            audio_sample_rate: sample_rate,
            audio_frame_size: frame_size,
            audio_channels: channels,
            video_pts: 0,
            audio_pts: 0,
            video_timebase,
            audio_timebase,
            font,
            start: Instant::now(),
            scaler: Scaler::default(),
            next_frame: None,
        })
    }

    pub fn set_realtime(&mut self, realtime: bool) {
        self.realtime = realtime;
    }

    pub fn from_stream(
        video_stream: &IngressStream,
        audio_stream: Option<&IngressStream>,
    ) -> Result<Self> {
        Self::from_stream_with_timebase(
            video_stream,
            audio_stream,
            AVRational {
                num: 1,
                den: video_stream.fps as i32,
            },
            audio_stream.map(|s| AVRational {
                num: 1,
                den: s.sample_rate as i32,
            }),
        )
    }

    pub fn from_stream_with_timebase(
        video_stream: &IngressStream,
        audio_stream: Option<&IngressStream>,
        video_timebase: AVRational,
        audio_timebase: Option<AVRational>,
    ) -> Result<Self> {
        Self::new(
            video_stream.fps,
            video_stream.width as _,
            video_stream.height as _,
            unsafe { transmute(video_stream.format as i32) },
            audio_stream.map(|i| i.sample_rate as _).unwrap_or(0),
            if audio_stream.is_none() { 0 } else { 1024 },
            audio_stream.map(|i| i.channels as _).unwrap_or(0),
            video_timebase,
            audio_timebase.unwrap_or(AVRational { num: 1, den: 1 }),
        )
    }

    pub unsafe fn from_av_streams(
        video_stream: *const AVStream,
        audio_stream: Option<*const AVStream>,
    ) -> Result<Self> {
        unsafe {
            if video_stream.is_null() {
                bail!("Video stream cannot be null");
            }

            let video_codec_par = (*video_stream).codecpar;
            let video_timebase = (*video_stream).time_base;

            // Extract video stream properties
            let width = (*video_codec_par).width as u16;
            let height = (*video_codec_par).height as u16;
            let pix_fmt = transmute((*video_codec_par).format);

            // Calculate FPS from timebase
            let fps = av_q2d((*video_stream).r_frame_rate) as f32;

            // Extract audio stream properties if available
            let (sample_rate, channels, audio_timebase) = if let Some(audio_stream) = audio_stream {
                if !audio_stream.is_null() {
                    let audio_codec_par = (*audio_stream).codecpar;
                    let audio_tb = (*audio_stream).time_base;
                    (
                        (*audio_codec_par).sample_rate as u32,
                        (*audio_codec_par).ch_layout.nb_channels as u8,
                        audio_tb,
                    )
                } else {
                    (0, 0, AVRational { num: 1, den: 44100 })
                }
            } else {
                (0, 0, AVRational { num: 1, den: 44100 })
            };

            let frame_size = if sample_rate > 0 { 1024 } else { 0 };
            Self::new(
                fps,
                width,
                height,
                pix_fmt,
                sample_rate,
                frame_size,
                channels,
                video_timebase,
                audio_timebase,
            )
        }
    }

    pub fn frame_no(&self) -> u64 {
        (self.video_pts / self.pts_per_frame()) as u64
    }

    /// Set the starting PTS values for video and audio
    pub fn set_starting_pts(&mut self, video_pts: i64, audio_pts: i64) {
        self.video_pts = video_pts;
        self.audio_pts = audio_pts;
        self.start = Instant::now().sub(Duration::from_secs_f64(
            video_pts as f64 / self.pts_per_frame() as f64 / self.fps as f64,
        ));
    }

    /// Create a new frame for composing text / images
    pub fn begin(&mut self) -> Result<()> {
        if self.next_frame.is_none() {
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
                (*src_frame).pts = self.video_pts;
                (*src_frame).duration = self.pts_per_frame() as _;
                (*src_frame).time_base = self.video_timebase;
                if av_frame_get_buffer(src_frame, 0) < 0 {
                    av_frame_free(&mut src_frame);
                    bail!("Failed to get frame buffer");
                }
                self.next_frame.replace(AvFrameRef::new(src_frame));
            }
        }
        Ok(())
    }

    /// Write some text into the next frame
    pub fn write_text(&mut self, msg: &str, size: f32, x: f32, y: f32) -> Result<()> {
        if self.next_frame.is_none() {
            bail!("Must call begin() before writing text")
        }
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.append(&[&self.font], &TextStyle::new(msg, size, 0));

        self.write_layout(layout, x, y)?;
        Ok(())
    }

    /// Write text with a black background and padding
    pub fn write_text_with_background(
        &mut self,
        msg: &str,
        size: f32,
        x: f32,
        y: f32,
        padding: f32,
    ) -> Result<()> {
        if self.next_frame.is_none() {
            bail!("Must call begin() before writing text")
        }
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.append(&[&self.font], &TextStyle::new(msg, size, 0));

        // Calculate text bounds
        let mut max_x: f32 = 0.0;
        let mut max_y: f32 = 0.0;
        for g in layout.glyphs() {
            let right = g.x + g.width as f32;
            let bottom = g.y + g.height as f32;
            if right > max_x {
                max_x = right;
            }
            if bottom > max_y {
                max_y = bottom;
            }
        }

        // Draw black background rectangle with padding
        let bg_x = (x - padding).max(0.0) as usize;
        let bg_y = (y - padding).max(0.0) as usize;
        let bg_width = (max_x + 2.0 * padding) as usize;
        let bg_height = (size * 0.25) as usize + (max_y + 2.0 * padding) as usize;

        unsafe {
            let Some(next_frame) = &self.next_frame else {
                bail!("Next frame must not be null");
            };
            let linesize = next_frame.linesize[0] as usize;
            let data = next_frame.data[0];

            for row in bg_y..(bg_y + bg_height).min(self.height as usize) {
                for col in bg_x..(bg_x + bg_width).min(self.width as usize) {
                    let offset = 4 * col + row * linesize;
                    let pixel = data.add(offset);
                    *pixel.offset(0) = 0; // R
                    *pixel.offset(1) = 0; // G
                    *pixel.offset(2) = 0; // B
                    *pixel.offset(3) = 255; // A
                }
            }
        }

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
                        let Some(next_frame) = &self.next_frame else {
                            bail!("Next frame must not be null");
                        };
                        let offset_dst = 4 * dst_x + dst_y * next_frame.linesize[0] as usize;
                        let pixel_dst = next_frame.data[0].add(offset_dst);
                        *pixel_dst.offset(0) = bitmap[offset_src];
                        *pixel_dst.offset(1) = bitmap[offset_src + 1];
                        *pixel_dst.offset(2) = bitmap[offset_src + 2];
                    }
                }
            }
        }
        Ok(())
    }

    pub unsafe fn fill_color(&mut self, color32: [u8; 4]) -> Result<()> {
        unsafe {
            let Some(next_frame) = &self.next_frame else {
                bail!("Must call begin() before writing frame data")
            };
            let buf = slice::from_raw_parts_mut(
                next_frame.data[0],
                self.width as usize * self.height as usize * 4,
            );
            for chunk in buf.chunks_exact_mut(4) {
                chunk[0] = color32[0];
                chunk[1] = color32[1];
                chunk[2] = color32[2];
                chunk[3] = color32[3];
            }
            Ok(())
        }
    }

    /// Copy data directly into the frame buffer (must be RGBA data)
    pub unsafe fn copy_frame_data(&mut self, data: &[u8]) -> Result<()> {
        unsafe {
            let Some(next_frame) = &self.next_frame else {
                bail!("Must call begin() before writing frame data")
            };
            let buf = slice::from_raw_parts_mut(
                next_frame.data[0],
                self.width as usize * self.height as usize * 4,
            );
            if buf.len() < data.len() {
                bail!("Frame buffer is too small");
            }
            buf.copy_from_slice(data);
            Ok(())
        }
    }

    fn pts_per_frame(&self) -> i64 {
        self.video_timebase.den as i64 / (self.video_timebase.num as i64 * self.fps as i64)
    }

    fn pts_of_nb_samples(&self, n: i64) -> i64 {
        let seconds = n as f64 / self.audio_sample_rate as f64;
        (seconds / unsafe { av_q2d(self.audio_timebase) }) as _
    }

    /// Generate audio to stay synchronized with video frames
    unsafe fn generate_audio_frame(&mut self) -> Result<Option<AvFrameRef>> {
        unsafe {
            const FREQUENCY: f32 = 440.0; // A4 note
            const PULSE_DURATION_MS: f32 = 50.0;
            const PULSE_PERIOD_MS: f32 = 1000.0;

            // audio is disabled if sample rate is 0
            if self.audio_sample_rate == 0 {
                return Ok(None);
            }

            // Calculate audio PTS needed to stay ahead of next video frame
            let next_video_pts = self.video_pts + self.pts_per_frame();

            // Convert video PTS to audio timebase to see how much audio we need
            let audio_pts_needed =
                av_rescale_q(next_video_pts, self.video_timebase, self.audio_timebase);

            // Generate audio if we don't have enough to cover the next video frame
            if self.audio_pts < audio_pts_needed {
                let audio_frame = av_frame_alloc();
                (*audio_frame).format = AV_SAMPLE_FMT_FLTP as _;
                (*audio_frame).nb_samples = self.audio_frame_size as _;
                (*audio_frame).duration = self.audio_frame_size as _;
                (*audio_frame).sample_rate = self.audio_sample_rate as _;
                (*audio_frame).pts = self.audio_pts;
                (*audio_frame).time_base = self.audio_timebase;
                (*audio_frame).duration = self.pts_of_nb_samples(self.audio_frame_size as _);
                av_channel_layout_default(&mut (*audio_frame).ch_layout, self.audio_channels as _);
                av_frame_get_buffer(audio_frame, 0);

                for ch in 0..self.audio_channels {
                    let data = (*audio_frame).data[ch as usize] as *mut f32;
                    for i in 0..self.audio_frame_size {
                        let sample_idx = self.audio_pts + i as i64;
                        let time_ms = (sample_idx as f32 / self.audio_sample_rate as f32) * 1000.0;
                        let time_in_period = time_ms % PULSE_PERIOD_MS;

                        let sample_value = if time_in_period < PULSE_DURATION_MS {
                            // During pulse: generate sine wave
                            let sample_time = sample_idx as f32 / self.audio_sample_rate as f32;
                            (2.0 * std::f32::consts::PI * FREQUENCY * sample_time).sin() * 0.1
                        } else {
                            // Silent period
                            0.0
                        };
                        *data.add(i as _) = sample_value;
                    }
                }

                return Ok(Some(AvFrameRef::new(audio_frame)));
            }

            Ok(None)
        }
    }

    /// Return the next frame for encoding (blocking)
    pub unsafe fn next(&mut self) -> Result<Option<AvFrameRef>> {
        unsafe {
            // set start time to now if this is the first call to next()
            if self.video_pts == 0 {
                self.start = Instant::now();
            }

            // try to get audio frames before video frames (non-blocking)
            let audio_frame = self.generate_audio_frame()?;
            if let Some(audio_frame) = audio_frame {
                self.audio_pts += audio_frame.duration;
                return Ok(Some(audio_frame));
            }

            // auto-init frame
            if self.next_frame.is_none() {
                self.begin()?;
            }

            if self.realtime {
                let stream_time = Duration::from_secs_f64(
                    self.video_pts as f64 / self.pts_per_frame() as f64 / self.fps as f64,
                );
                let real_time = self.start.elapsed();
                let wait_time = if stream_time > real_time {
                    stream_time - real_time
                } else {
                    Duration::new(0, 0)
                };
                if !wait_time.is_zero() && wait_time.as_secs_f32() > 1f32 / self.fps {
                    std::thread::sleep(wait_time);
                }
            }
            let Some(next_frame) = self.next_frame.take() else {
                return Ok(None);
            };

            // convert to output pixel format, or just return internal frame if it matches output
            if self.video_sample_fmt != transmute(next_frame.format) {
                let out_frame = self.scaler.process_frame(
                    &next_frame,
                    self.width,
                    self.height,
                    self.video_sample_fmt,
                )?;
                self.video_pts += next_frame.duration;
                Ok(Some(out_frame))
            } else {
                self.video_pts += next_frame.duration;
                Ok(Some(next_frame))
            }
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

            let mut frame_gen = FrameGenerator::new(
                fps,
                1280,
                720,
                AV_PIX_FMT_YUV420P,
                sample_rate,
                frame_size,
                channels,
                AVRational {
                    num: 1,
                    den: fps as i32,
                },
                AVRational {
                    num: 1,
                    den: sample_rate as i32,
                },
            )
            .unwrap();

            let samples_per_frame = sample_rate as f64 / fps as f64; // Expected: 1470 samples per frame
            println!("Expected samples per video frame: {:.2}", samples_per_frame);

            let mut audio_frames = 0;
            let mut video_frames = 0;
            let mut total_audio_samples = 0;

            // Generate frames for 2 seconds (60 video frames at 30fps)
            for i in 0..120 {
                let mut frame = frame_gen.next().unwrap().unwrap();

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

                    println!(
                        "Frame {}: VIDEO - PTS: {}, frame_idx: {}, expected_audio: {}, actual_audio: {}, deficit: {}",
                        i,
                        (*frame).pts,
                        video_frames,
                        expected_audio_samples,
                        total_audio_samples,
                        audio_deficit
                    );

                    // Verify we have enough audio for this video frame
                    assert!(
                        total_audio_samples >= expected_audio_samples,
                        "Video frame {} needs {} audio samples but only have {}",
                        video_frames,
                        expected_audio_samples,
                        total_audio_samples
                    );
                }
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

            let mut frame_gen = FrameGenerator::new(
                fps,
                1280,
                720,
                AV_PIX_FMT_YUV420P,
                sample_rate,
                1024,
                2,
                AVRational {
                    num: 1,
                    den: fps as i32,
                },
                AVRational {
                    num: 1,
                    den: sample_rate as i32,
                },
            )
            .unwrap();

            let mut last_audio_pts = -1i64;
            let mut last_video_pts = -1i64;
            let mut audio_pts_gaps = Vec::new();
            let mut video_pts_gaps = Vec::new();

            // Generate 60 frames to test PTS progression
            for _ in 0..60 {
                let mut frame = frame_gen.next().unwrap().unwrap();

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
