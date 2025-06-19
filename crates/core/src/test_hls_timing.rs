use crate::generator::FrameGenerator;
use crate::mux::{HlsMuxer, SegmentType};
use crate::variant::audio::AudioVariant;
use crate::variant::mapping::VariantMapping;
use crate::variant::video::VideoVariant;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{Context, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_q2d, AVMediaType::AVMEDIA_TYPE_AUDIO, AVMediaType::AVMEDIA_TYPE_VIDEO,
    AVPixelFormat::AV_PIX_FMT_YUV420P, AVRational, AVSampleFormat::AV_SAMPLE_FMT_FLTP,
    AV_NOPTS_VALUE, AV_PROFILE_H264_MAIN,
};
use ffmpeg_rs_raw::{Demuxer, Encoder};
use m3u8_rs::{parse_media_playlist, MediaSegmentType};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct HlsTimingResult {
    pub playlist_duration: f32,
    pub actual_duration: f64,
    pub video_duration: f64,
    pub audio_duration: f64,
    pub difference: f64,
    pub segment_name: String,
    pub is_partial: bool,
    pub independent: bool,
}

#[derive(Debug)]
pub struct HlsTimingTestResult {
    pub total_segments: usize,
    pub full_segments: usize,
    pub partial_segments: usize,
    pub independent_partials: usize,
    pub total_playlist_duration: f32,
    pub total_actual_duration: f64,
    pub total_difference: f64,
    pub average_difference: f64,
    pub min_difference: f64,
    pub max_difference: f64,
    pub problematic_segments: Vec<HlsTimingResult>,
    pub segments: Vec<HlsTimingResult>,
    pub test_duration: Duration,
    pub success: bool,
    pub error_message: Option<String>,
}

impl HlsTimingTestResult {
    /// Check if the HLS timing test passed based on thresholds
    pub fn passes(&self, max_avg_diff: f64, max_individual_diff: f64) -> bool {
        self.success
            && self.average_difference.abs() <= max_avg_diff
            && self
                .problematic_segments
                .iter()
                .all(|s| s.difference.abs() <= max_individual_diff)
    }

    /// Get a summary of the test results
    pub fn summary(&self) -> String {
        if !self.success {
            return format!(
                "FAILED: {}",
                self.error_message.as_deref().unwrap_or("Unknown error")
            );
        }

        format!(
            "PASSED: {} segments, avg diff: {:.3}s, {} problematic",
            self.total_segments,
            self.average_difference,
            self.problematic_segments.len()
        )
    }
}

pub struct HlsTimingTester {
    max_avg_difference: f64,
    max_individual_difference: f64,
    problematic_threshold: f64,
}

impl Default for HlsTimingTester {
    fn default() -> Self {
        Self {
            max_avg_difference: 0.1,        // 100ms average difference
            max_individual_difference: 0.5, // 500ms individual difference
            problematic_threshold: 0.2,     // 200ms considered problematic
        }
    }
}

impl HlsTimingTester {
    pub fn new(max_avg_diff: f64, max_individual_diff: f64, problematic_threshold: f64) -> Self {
        Self {
            max_avg_difference: max_avg_diff,
            max_individual_difference: max_individual_diff,
            problematic_threshold,
        }
    }

    /// Generate and test HLS stream with test pattern
    pub fn test_generated_stream(
        &self,
        output_dir: &Path,
        duration_seconds: f32,
        segment_type: SegmentType,
    ) -> Result<HlsTimingTestResult> {
        let start_time = Instant::now();

        // Generate test stream
        let stream_id = Uuid::new_v4();
        let hls_dir =
            self.generate_test_stream(output_dir, &stream_id, duration_seconds, segment_type)?;

        // Test the generated stream
        match self.test_stream_timing_internal(&hls_dir) {
            Ok(mut result) => {
                result.test_duration = start_time.elapsed();
                result.success =
                    result.passes(self.max_avg_difference, self.max_individual_difference);
                Ok(result)
            }
            Err(e) => Ok(HlsTimingTestResult {
                total_segments: 0,
                full_segments: 0,
                partial_segments: 0,
                independent_partials: 0,
                total_playlist_duration: 0.0,
                total_actual_duration: 0.0,
                total_difference: 0.0,
                average_difference: 0.0,
                min_difference: 0.0,
                max_difference: 0.0,
                problematic_segments: Vec::new(),
                segments: Vec::new(),
                test_duration: start_time.elapsed(),
                success: false,
                error_message: Some(e.to_string()),
            }),
        }
    }

    /// Generate test HLS stream with test pattern
    fn generate_test_stream(
        &self,
        output_dir: &Path,
        stream_id: &Uuid,
        duration_seconds: f32,
        segment_type: SegmentType,
    ) -> Result<PathBuf> {
        const VIDEO_FPS: f32 = 30.0;
        const VIDEO_WIDTH: u16 = 1280;
        const VIDEO_HEIGHT: u16 = 720;
        const SAMPLE_RATE: u32 = 44100;

        // Create video encoder
        let mut video_encoder = unsafe {
            Encoder::new_with_name("libx264")?
                .with_stream_index(0)
                .with_framerate(VIDEO_FPS)?
                .with_bitrate(1_000_000)
                .with_pix_fmt(AV_PIX_FMT_YUV420P)
                .with_width(VIDEO_WIDTH as _)
                .with_height(VIDEO_HEIGHT as _)
                .with_level(51)
                .with_profile(AV_PROFILE_H264_MAIN)
                .open(None)?
        };

        // Create audio encoder
        let mut audio_encoder = unsafe {
            Encoder::new_with_name("aac")?
                .with_stream_index(1)
                .with_default_channel_layout(1)
                .with_bitrate(128_000)
                .with_sample_format(AV_SAMPLE_FMT_FLTP)
                .with_sample_rate(SAMPLE_RATE as _)?
                .open(None)?
        };

        // Create variant streams
        let video_stream = VideoVariant {
            mapping: VariantMapping {
                id: Uuid::new_v4(),
                src_index: 0,
                dst_index: 0,
                group_id: 0,
            },
            width: VIDEO_WIDTH,
            height: VIDEO_HEIGHT,
            fps: VIDEO_FPS,
            bitrate: 1_000_000,
            codec: "libx264".to_string(),
            profile: AV_PROFILE_H264_MAIN as usize,
            level: 51,
            keyframe_interval: 60,
            pixel_format: AV_PIX_FMT_YUV420P as u32,
        };

        let audio_stream = AudioVariant {
            mapping: VariantMapping {
                id: Uuid::new_v4(),
                src_index: 1,
                dst_index: 1,
                group_id: 0,
            },
            bitrate: 128_000,
            codec: "aac".to_string(),
            channels: 1,
            sample_rate: SAMPLE_RATE as usize,
            sample_fmt: "fltp".to_string(),
        };

        let video_variant = VariantStream::Video(video_stream.clone());
        let audio_variant = VariantStream::Audio(audio_stream.clone());
        let variants = vec![
            (&video_variant, &video_encoder),
            (&audio_variant, &audio_encoder),
        ];

        // Create HLS muxer
        let mut hls_muxer =
            HlsMuxer::new(output_dir.to_path_buf(), variants.into_iter(), segment_type)?;

        // Create frame generator
        let frame_size = unsafe { (*audio_encoder.codec_context()).frame_size as _ };
        let mut frame_gen = FrameGenerator::new(
            VIDEO_FPS,
            VIDEO_WIDTH,
            VIDEO_HEIGHT,
            AV_PIX_FMT_YUV420P,
            SAMPLE_RATE,
            frame_size,
            1,
            AVRational {
                num: 1,
                den: VIDEO_FPS as i32,
            },
            AVRational {
                num: 1,
                den: SAMPLE_RATE as i32,
            },
        )?;
        frame_gen.set_realtime(false);

        // Generate frames for the specified duration
        let total_video_frames = (duration_seconds * VIDEO_FPS) as u64;
        let mut video_frames_generated = 0;

        while video_frames_generated < total_video_frames {
            unsafe {
                frame_gen.begin()?;
                frame_gen.write_text(
                    &format!("Video Frame: {}", video_frames_generated),
                    40.0,
                    50.0,
                    50.0,
                )?;
                frame_gen.write_text(
                    &format!("Time: {:.1}s", video_frames_generated as f32 / VIDEO_FPS),
                    40.0,
                    50.0,
                    100.0,
                )?;

                let mut frame = frame_gen.next()?;
                if frame.is_null() {
                    log::warn!("FrameGenerator returned null frame unexpectedly");
                    break;
                }

                // Determine if this is audio or video frame and encode accordingly
                if (*frame).sample_rate > 0 {
                    // Audio frame - don't increment video counter
                    log::debug!("Generated audio frame, PTS: {}", (*frame).pts);
                    for mut pkt in audio_encoder.encode_frame(frame)? {
                        let result = hls_muxer.mux_packet(pkt, &audio_stream.id())?;
                        if let crate::egress::EgressResult::Segments {
                            created,
                            deleted: _,
                        } = result
                        {
                            for segment in created {
                                log::debug!("Created audio segment: {:?}", segment.path);
                            }
                        }
                        ffmpeg_rs_raw::ffmpeg_sys_the_third::av_packet_free(&mut pkt);
                    }
                } else {
                    // Video frame - increment video counter
                    log::debug!(
                        "Generated video frame {}, PTS: {}",
                        video_frames_generated,
                        (*frame).pts
                    );
                    for mut pkt in video_encoder.encode_frame(frame)? {
                        let result = hls_muxer.mux_packet(pkt, &video_stream.id())?;
                        if let crate::egress::EgressResult::Segments {
                            created,
                            deleted: _,
                        } = result
                        {
                            for segment in created {
                                log::debug!("Created video segment: {:?}", segment.path);
                            }
                        }
                        ffmpeg_rs_raw::ffmpeg_sys_the_third::av_packet_free(&mut pkt);
                    }
                    video_frames_generated += 1;
                }

                ffmpeg_rs_raw::ffmpeg_sys_the_third::av_frame_free(&mut frame);
            }
        }

        // Flush encoders to ensure all packets are written
        unsafe {
            // Flush video encoder
            for mut pkt in video_encoder.encode_frame(std::ptr::null_mut())? {
                hls_muxer.mux_packet(pkt, &video_stream.id())?;
                ffmpeg_rs_raw::ffmpeg_sys_the_third::av_packet_free(&mut pkt);
            }

            // Flush audio encoder
            for mut pkt in audio_encoder.encode_frame(std::ptr::null_mut())? {
                hls_muxer.mux_packet(pkt, &audio_stream.id())?;
                ffmpeg_rs_raw::ffmpeg_sys_the_third::av_packet_free(&mut pkt);
            }
        }

        log::info!(
            "Generated {} video frames ({:.1}s) of test HLS stream at",
            video_frames_generated,
            video_frames_generated as f32 / VIDEO_FPS
        );

        Ok(output_dir.join(stream_id.to_string()).join("stream_0"))
    }

    /// Test HLS timing for a specific stream directory
    pub fn test_stream_timing(&self, hls_dir: &Path) -> HlsTimingTestResult {
        let start_time = Instant::now();

        match self.test_stream_timing_internal(hls_dir) {
            Ok(mut result) => {
                result.test_duration = start_time.elapsed();
                result.success =
                    result.passes(self.max_avg_difference, self.max_individual_difference);
                result
            }
            Err(e) => HlsTimingTestResult {
                total_segments: 0,
                full_segments: 0,
                partial_segments: 0,
                independent_partials: 0,
                total_playlist_duration: 0.0,
                total_actual_duration: 0.0,
                total_difference: 0.0,
                average_difference: 0.0,
                min_difference: 0.0,
                max_difference: 0.0,
                problematic_segments: Vec::new(),
                segments: Vec::new(),
                test_duration: start_time.elapsed(),
                success: false,
                error_message: Some(e.to_string()),
            },
        }
    }

    fn test_stream_timing_internal(&self, hls_dir: &Path) -> Result<HlsTimingTestResult> {
        let playlist_path = hls_dir.join("live.m3u8");

        if !playlist_path.exists() {
            return Err(anyhow::anyhow!(
                "Playlist file does not exist: {:?}",
                playlist_path
            ));
        }

        // Parse the playlist
        let playlist_content =
            fs::read_to_string(&playlist_path).context("Failed to read playlist file")?;

        let (_, playlist) = parse_media_playlist(playlist_content.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to parse playlist: {:?}", e))?;

        let mut segments = Vec::new();
        let mut total_playlist_duration = 0.0f32;
        let mut total_actual_duration = 0.0f64;

        // Analyze each segment
        for segment_type in &playlist.segments {
            match segment_type {
                MediaSegmentType::Full(segment) => {
                    let segment_path = hls_dir.join(&segment.uri);

                    if !segment_path.exists() {
                        continue; // Skip missing segments
                    }

                    let durations = self.analyze_segment(&segment_path)?;
                    let actual_duration = durations.total_duration;
                    let video_duration = durations.video_duration;
                    let audio_duration = durations.audio_duration;
                    let playlist_duration = segment.duration;
                    let difference = actual_duration - playlist_duration as f64;

                    let result = HlsTimingResult {
                        playlist_duration,
                        actual_duration,
                        video_duration,
                        audio_duration,
                        difference,
                        segment_name: segment.uri.clone(),
                        is_partial: false,
                        independent: false,
                    };

                    segments.push(result);
                    total_playlist_duration += playlist_duration;
                    total_actual_duration += actual_duration;
                }
                MediaSegmentType::Partial(partial) => {
                    let segment_path = hls_dir.join(&partial.uri);

                    if !segment_path.exists() {
                        continue; // Skip missing segments
                    }

                    let durations = if let Some(byte_range) = &partial.byte_range {
                        self.analyze_partial_segment(
                            &segment_path,
                            byte_range.length,
                            byte_range.offset,
                        )?
                    } else {
                        self.analyze_segment(&segment_path)?
                    };

                    let actual_duration = durations.total_duration;
                    let video_duration = durations.video_duration;
                    let audio_duration = durations.audio_duration;
                    let playlist_duration = partial.duration as f32;
                    let difference = actual_duration - playlist_duration as f64;

                    let result = HlsTimingResult {
                        playlist_duration,
                        actual_duration,
                        video_duration,
                        audio_duration,
                        difference,
                        segment_name: partial.uri.clone(),
                        is_partial: true,
                        independent: partial.independent,
                    };

                    segments.push(result);
                    total_playlist_duration += playlist_duration;
                    total_actual_duration += actual_duration;
                }
                MediaSegmentType::PreloadHint(_) => {
                    // Skip preload hints
                    continue;
                }
            }
        }

        // Calculate statistics
        let full_segments = segments.iter().filter(|s| !s.is_partial).count();
        let partial_segments = segments.iter().filter(|s| s.is_partial).count();
        let independent_partials = segments
            .iter()
            .filter(|s| s.is_partial && s.independent)
            .count();
        let total_difference = total_actual_duration - total_playlist_duration as f64;
        let average_difference = if !segments.is_empty() {
            total_difference / segments.len() as f64
        } else {
            0.0
        };

        let differences: Vec<f64> = segments.iter().map(|s| s.difference).collect();
        let min_difference = differences.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max_difference = differences.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));

        // Find problematic segments
        let problematic_segments: Vec<HlsTimingResult> = segments
            .iter()
            .filter(|s| s.difference.abs() > self.problematic_threshold)
            .cloned()
            .collect();

        Ok(HlsTimingTestResult {
            total_segments: segments.len(),
            full_segments,
            partial_segments,
            independent_partials,
            total_playlist_duration,
            total_actual_duration,
            total_difference,
            average_difference,
            min_difference,
            max_difference,
            problematic_segments,
            segments,
            test_duration: Duration::from_secs(0), // Will be set by caller
            success: true,                         // Will be determined by caller
            error_message: None,
        })
    }

    /// Test multiple HLS streams concurrently
    pub async fn test_multiple_streams(
        &self,
        hls_dirs: Vec<PathBuf>,
    ) -> HashMap<PathBuf, HlsTimingTestResult> {
        let mut results = HashMap::new();

        // Run tests concurrently
        let futures: Vec<_> = hls_dirs
            .into_iter()
            .map(|dir| {
                let tester = HlsTimingTester::new(
                    self.max_avg_difference,
                    self.max_individual_difference,
                    self.problematic_threshold,
                );
                let dir_clone = dir.clone();
                async move {
                    let result =
                        tokio::task::spawn_blocking(move || tester.test_stream_timing(&dir_clone))
                            .await
                            .unwrap_or_else(|_| HlsTimingTestResult {
                                total_segments: 0,
                                full_segments: 0,
                                partial_segments: 0,
                                independent_partials: 0,
                                total_playlist_duration: 0.0,
                                total_actual_duration: 0.0,
                                total_difference: 0.0,
                                average_difference: 0.0,
                                min_difference: 0.0,
                                max_difference: 0.0,
                                problematic_segments: Vec::new(),
                                segments: Vec::new(),
                                test_duration: Duration::from_secs(0),
                                success: false,
                                error_message: Some("Task panicked".to_string()),
                            });
                    (dir, result)
                }
            })
            .collect();

        let resolved_futures = futures::future::join_all(futures).await;

        for (dir, result) in resolved_futures {
            results.insert(dir, result);
        }

        results
    }

    fn analyze_segment(&self, path: &Path) -> Result<SegmentDurations> {
        let file = fs::File::open(path)
            .with_context(|| format!("Failed to open file: {}", path.display()))?;
        self.analyze_segment_with_reader(Box::new(file))
    }

    fn analyze_partial_segment(
        &self,
        path: &Path,
        length: u64,
        offset: Option<u64>,
    ) -> Result<SegmentDurations> {
        let reader = ByteRangeReader::new(path, length, offset)?;
        self.analyze_segment_with_reader(Box::new(reader))
    }

    fn analyze_segment_with_reader(&self, reader: Box<dyn Read>) -> Result<SegmentDurations> {
        let mut demuxer = Demuxer::new_custom_io(reader, None)?;

        unsafe {
            demuxer.probe_input()?;
        }

        let mut video_start_pts = AV_NOPTS_VALUE;
        let mut video_end_pts = AV_NOPTS_VALUE;
        let mut audio_start_pts = AV_NOPTS_VALUE;
        let mut audio_end_pts = AV_NOPTS_VALUE;
        let mut video_last_duration = 0i64;
        let mut audio_last_duration = 0i64;
        let mut video_stream_idx: Option<usize> = None;
        let mut audio_stream_idx: Option<usize> = None;

        // Read all packets and track timing
        loop {
            let packet_result = unsafe { demuxer.get_packet() };
            match packet_result {
                Ok((pkt, stream)) => {
                    if pkt.is_null() {
                        break;
                    }

                    unsafe {
                        let codec_type = (*(*stream).codecpar).codec_type;
                        let pts = (*pkt).pts;
                        let duration = (*pkt).duration;
                        let current_stream_idx = (*stream).index as usize;

                        match codec_type {
                            AVMEDIA_TYPE_VIDEO => {
                                if video_stream_idx.is_none() {
                                    video_stream_idx = Some(current_stream_idx);
                                }
                                if pts != AV_NOPTS_VALUE {
                                    if video_start_pts == AV_NOPTS_VALUE {
                                        video_start_pts = pts;
                                    }
                                    video_end_pts = pts;
                                    video_last_duration = duration;
                                }
                            }
                            AVMEDIA_TYPE_AUDIO => {
                                if audio_stream_idx.is_none() {
                                    audio_stream_idx = Some(current_stream_idx);
                                }
                                if pts != AV_NOPTS_VALUE {
                                    if audio_start_pts == AV_NOPTS_VALUE {
                                        audio_start_pts = pts;
                                    }
                                    audio_end_pts = pts;
                                    audio_last_duration = duration;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Calculate durations
        let video_duration = if let Some(stream_idx) = video_stream_idx {
            if video_start_pts != AV_NOPTS_VALUE && video_end_pts != AV_NOPTS_VALUE {
                unsafe {
                    let stream = demuxer.get_stream(stream_idx)?;
                    let time_base = (*stream).time_base;
                    let pts_duration = (video_end_pts - video_start_pts) as f64 * av_q2d(time_base);
                    let last_pkt_duration = video_last_duration as f64 * av_q2d(time_base);
                    pts_duration + last_pkt_duration
                }
            } else {
                0.0
            }
        } else {
            0.0
        };

        let audio_duration = if let Some(stream_idx) = audio_stream_idx {
            if audio_start_pts != AV_NOPTS_VALUE && audio_end_pts != AV_NOPTS_VALUE {
                unsafe {
                    let stream = demuxer.get_stream(stream_idx)?;
                    let time_base = (*stream).time_base;
                    let pts_duration = (audio_end_pts - audio_start_pts) as f64 * av_q2d(time_base);
                    let last_pkt_duration = audio_last_duration as f64 * av_q2d(time_base);
                    pts_duration + last_pkt_duration
                }
            } else {
                0.0
            }
        } else {
            0.0
        };

        let total_duration = video_duration.max(audio_duration);

        Ok(SegmentDurations {
            total_duration,
            video_duration,
            audio_duration,
        })
    }
}

#[derive(Debug)]
struct SegmentDurations {
    total_duration: f64,
    video_duration: f64,
    audio_duration: f64,
}

/// Custom IO reader for byte range access
struct ByteRangeReader {
    file: fs::File,
    start_offset: u64,
    length: u64,
    current_pos: u64,
}

impl ByteRangeReader {
    fn new(path: &Path, length: u64, offset: Option<u64>) -> Result<Self> {
        use std::io::{Seek, SeekFrom};

        let mut file = fs::File::open(path)
            .with_context(|| format!("Failed to open file: {}", path.display()))?;

        let start_offset = offset.unwrap_or(0);
        file.seek(SeekFrom::Start(start_offset))
            .with_context(|| format!("Failed to seek to offset {}", start_offset))?;

        Ok(ByteRangeReader {
            file,
            start_offset,
            length,
            current_pos: 0,
        })
    }
}

impl Read for ByteRangeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.length - self.current_pos;
        if remaining == 0 {
            return Ok(0);
        }

        let to_read = std::cmp::min(buf.len() as u64, remaining) as usize;
        let bytes_read = self.file.read(&mut buf[..to_read])?;
        self.current_pos += bytes_read as u64;
        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_timing_tester_creation() {
        let tester = HlsTimingTester::default();
        assert_eq!(tester.max_avg_difference, 0.1);
        assert_eq!(tester.max_individual_difference, 0.5);
        assert_eq!(tester.problematic_threshold, 0.2);
    }

    #[test]
    fn test_timing_result_passes() {
        let result = HlsTimingTestResult {
            total_segments: 10,
            full_segments: 8,
            partial_segments: 2,
            independent_partials: 1,
            total_playlist_duration: 20.0,
            total_actual_duration: 20.05,
            total_difference: 0.05,
            average_difference: 0.005,
            min_difference: -0.01,
            max_difference: 0.02,
            problematic_segments: Vec::new(),
            segments: Vec::new(),
            test_duration: Duration::from_millis(100),
            success: true,
            error_message: None,
        };

        assert!(result.passes(0.1, 0.5));
        assert!(!result.passes(0.001, 0.5));
    }

    #[test]
    fn test_missing_playlist() {
        let temp_dir = tempdir().unwrap();
        let tester = HlsTimingTester::default();
        let result = tester.test_stream_timing(temp_dir.path());

        assert!(!result.success);
        assert!(result.error_message.is_some());
        assert!(result.error_message.unwrap().contains("does not exist"));
    }

    #[test]
    fn test_generated_hls_stream_mpegts() {
        env_logger::try_init().ok();

        let temp_dir = tempdir().unwrap();
        let tester = HlsTimingTester::new(0.2, 1.0, 0.5); // More lenient thresholds for test

        let result = tester.test_generated_stream(
            temp_dir.path(),
            10.0, // 10 seconds
            SegmentType::MPEGTS,
        );

        match result {
            Ok(test_result) => {
                assert!(
                    test_result.success,
                    "Test should pass: {}",
                    test_result.summary()
                );
                assert!(
                    test_result.total_segments > 0,
                    "Should have generated segments"
                );
                assert!(
                    test_result.total_playlist_duration > 8.0,
                    "Should have ~10s of content"
                );
                assert!(test_result.full_segments > 0, "Should have full segments");
                println!("✓ MPEG-TS test passed: {}", test_result.summary());
            }
            Err(e) => {
                panic!("Test generation failed: {}", e);
            }
        }
    }

    #[ignore]
    #[test]
    fn test_generated_hls_stream_fmp4() {
        env_logger::try_init().ok();

        let temp_dir = tempdir().unwrap();
        let tester = HlsTimingTester::new(0.2, 1.0, 0.5); // More lenient thresholds for test

        let result = tester.test_generated_stream(
            temp_dir.path(),
            8.0, // 8 seconds
            SegmentType::FMP4,
        );

        match result {
            Ok(test_result) => {
                assert!(
                    test_result.success,
                    "Test should pass: {}",
                    test_result.summary()
                );
                assert!(
                    test_result.total_segments > 0,
                    "Should have generated segments"
                );
                assert!(
                    test_result.total_playlist_duration > 6.0,
                    "Should have ~8s of content"
                );
                assert!(test_result.full_segments > 0, "Should have full segments");
                println!("✓ fMP4 test passed: {}", test_result.summary());
            }
            Err(e) => {
                panic!("Test generation failed: {}", e);
            }
        }
    }

    #[test]
    fn test_30_second_stream() {
        env_logger::try_init().ok();

        let temp_dir = tempdir().unwrap();
        let tester = HlsTimingTester::default();

        let result = tester.test_generated_stream(
            temp_dir.path(),
            30.0, // 30 seconds as requested
            SegmentType::MPEGTS,
        );

        match result {
            Ok(test_result) => {
                println!("{:?}", test_result);
                println!("30-second stream test results:");
                println!("  Total segments: {}", test_result.total_segments);
                println!("  Full segments: {}", test_result.full_segments);
                println!("  Partial segments: {}", test_result.partial_segments);
                println!(
                    "  Total playlist duration: {:.1}s",
                    test_result.total_playlist_duration
                );
                println!(
                    "  Total actual duration: {:.1}s",
                    test_result.total_actual_duration
                );
                println!(
                    "  Average difference: {:.3}s",
                    test_result.average_difference
                );
                println!("  Test duration: {:?}", test_result.test_duration);
                println!("  Result: {}", test_result.summary());

                assert!(
                    test_result.success,
                    "30s test should pass: {}",
                    test_result.summary()
                );
                assert!(
                    test_result.total_segments >= 2,
                    "Should have multiple segments for 30s"
                );
                assert!(
                    test_result.total_playlist_duration >= 25.0,
                    "Should have ~30s of content"
                );

                if !test_result.problematic_segments.is_empty() {
                    println!("  Problematic segments:");
                    for seg in &test_result.problematic_segments {
                        println!(
                            "    {}: {:.3}s difference",
                            seg.segment_name, seg.difference
                        );
                    }
                }
            }
            Err(e) => {
                panic!("30-second test generation failed: {}", e);
            }
        }
    }
}
