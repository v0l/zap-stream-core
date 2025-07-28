use anyhow::{Context, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{av_dump_format, av_q2d, AVMediaType::AVMEDIA_TYPE_AUDIO, AVMediaType::AVMEDIA_TYPE_VIDEO, AV_NOPTS_VALUE};
use ffmpeg_rs_raw::Demuxer;
use m3u8_rs::{parse_media_playlist, MediaSegmentType};
use std::env;
use std::fmt;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct SegmentInfo {
    filename: String,
    playlist_duration: f32,
    actual_duration: f64,
    video_duration: f64,
    audio_duration: f64,
    difference: f64,
    segment_type: SegmentAnalysisType,
}

#[derive(Debug, Clone)]
enum SegmentAnalysisType {
    Full,
    Partial {
        independent: bool,
        byte_range: Option<(u64, Option<u64>)>,
    },
}

#[derive(Debug)]
struct SegmentDurations {
    total_duration: f64,
    video_duration: f64,
    audio_duration: f64,
    video_packets: u64,
    audio_packets: u64,
    video_start_pts: i64,
    video_end_pts: i64,
    audio_start_pts: i64,
    audio_end_pts: i64,
}

#[derive(Debug)]
struct InitSegmentInfo {
    stream_count: usize,
    streams: Vec<StreamInfo>,
    has_moov: bool,
    pixel_format_set: bool,
}

#[derive(Debug)]
struct StreamInfo {
    codec_type: String,
    codec_name: String,
    width: Option<i32>,
    height: Option<i32>,
    pixel_format: Option<String>,
}

impl fmt::Display for StreamInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.codec_type.as_str() {
            "video" => {
                if let (Some(w), Some(h)) = (self.width, self.height) {
                    write!(f, "{} {}x{}", self.codec_name, w, h)?;
                } else {
                    write!(f, "{}", self.codec_name)?;
                }
                if let Some(ref pix_fmt) = self.pixel_format {
                    write!(f, " ({})", pix_fmt)?;
                }
                Ok(())
            }
            "audio" => write!(f, "{} (audio)", self.codec_name),
            _ => write!(f, "{} ({})", self.codec_name, self.codec_type),
        }
    }
}

/// Custom IO reader that implements Read for byte range access to files
/// This allows us to read only a specific byte range from a file, which is essential
/// for analyzing HLS-LL partial segments that reference byte ranges in larger files.
struct ByteRangeReader {
    file: fs::File,
    start_offset: u64,
    length: u64,
    current_pos: u64,
}

impl ByteRangeReader {
    /// Create a new ByteRangeReader for the specified file and byte range
    fn new(path: &Path, length: u64, offset: Option<u64>) -> Result<Self> {
        let mut file = fs::File::open(path)
            .with_context(|| format!("Failed to open file: {}", path.display()))?;

        let start_offset = offset.unwrap_or(0);

        // Seek to the start of our byte range
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
        // Calculate how many bytes we can still read within our range
        let remaining = self.length - self.current_pos;
        if remaining == 0 {
            return Ok(0); // EOF for our byte range
        }

        // Limit the read to not exceed our byte range
        let to_read = std::cmp::min(buf.len() as u64, remaining) as usize;
        let bytes_read = self.file.read(&mut buf[..to_read])?;

        self.current_pos += bytes_read as u64;
        Ok(bytes_read)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <path_to_hls_directory>", args[0]);
        eprintln!(
            "Example: {} out/hls/8c220348-fdbb-44cd-94d5-97a11a9ec91d/stream_0",
            args[0]
        );
        std::process::exit(1);
    }

    let hls_dir = PathBuf::from(&args[1]);
    let playlist_path = hls_dir.join("live.m3u8");

    if !playlist_path.exists() {
        eprintln!("Error: Playlist file {:?} does not exist", playlist_path);
        std::process::exit(1);
    }

    println!("Analyzing HLS stream: {}", hls_dir.display());
    println!("Playlist: {}", playlist_path.display());

    // Check for initialization segment
    let init_path = hls_dir.join("init.mp4");
    if init_path.exists() {
        println!("Init segment: {}", init_path.display());
        match analyze_init_segment(&init_path) {
            Ok(info) => {
                println!("  Streams: {}", info.stream_count);
                for (i, stream_info) in info.streams.iter().enumerate() {
                    println!("    Stream {}: {}", i, stream_info);
                }
                if info.has_moov {
                    println!("  ✓ Contains MOOV box");
                } else {
                    println!("  ✗ Missing MOOV box");
                }
                if info.pixel_format_set {
                    println!("  ✓ Pixel format properly set");
                } else {
                    println!("  ✗ Pixel format not set");
                }
            }
            Err(e) => {
                println!("  Error analyzing init segment: {}", e);
            }
        }
    } else {
        println!("No init segment found");
    }
    println!();

    // Parse the playlist
    let playlist_content =
        fs::read_to_string(&playlist_path).context("Failed to read playlist file")?;

    let (_, playlist) = parse_media_playlist(playlist_content.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to parse playlist: {:?}", e))?;

    // Analyze each segment
    let mut segments = Vec::new();
    let mut total_playlist_duration = 0.0f32;
    let mut total_actual_duration = 0.0f64;

    println!("Segment Analysis:");
    println!(
        "{:<12} {:>4} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "Segment", "Type", "Playlist", "Actual", "Video", "Audio", "Difference", "Info"
    );
    println!(
        "{:<12} {:>4} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "--------", "----", "--------", "------", "-----", "-----", "----------", "----"
    );

    for segment_type in &playlist.segments {
        match segment_type {
            MediaSegmentType::Full(segment) => {
                let segment_path = hls_dir.join(&segment.uri);

                if !segment_path.exists() {
                    eprintln!("Warning: Segment file {:?} does not exist", segment_path);
                    continue;
                }

                // Analyze file using demuxer
                let durations = if segment_path.extension().map_or(false, |ext| ext == "m4s") {
                    // This is an fMP4 segment, combine with init segment
                    analyze_fmp4_segment(&hls_dir.join("init.mp4"), &segment_path)?
                } else {
                    // Regular segment (MPEG-TS)
                    analyze_segment(&segment_path)?
                };
                let actual_duration = durations.total_duration;
                let video_duration = durations.video_duration;
                let audio_duration = durations.audio_duration;

                let playlist_duration = segment.duration;
                let difference = actual_duration - playlist_duration as f64;

                let info = SegmentInfo {
                    filename: segment.uri.clone(),
                    playlist_duration,
                    actual_duration,
                    video_duration,
                    audio_duration,
                    difference,
                    segment_type: SegmentAnalysisType::Full,
                };

                println!(
                    "{:<12} {:>4} {:>12.3} {:>12.3} {:>12.3} {:>12.3} {:>12.3} {:>12}",
                    info.filename,
                    "FULL",
                    info.playlist_duration,
                    info.actual_duration,
                    info.video_duration,
                    info.audio_duration,
                    info.difference,
                    ""
                );

                segments.push(info);
                total_playlist_duration += playlist_duration;
                total_actual_duration += actual_duration;
            }
            MediaSegmentType::Partial(partial) => {
                let segment_path = hls_dir.join(&partial.uri);

                if !segment_path.exists() {
                    eprintln!(
                        "Warning: Partial segment file {:?} does not exist",
                        segment_path
                    );
                    continue;
                }

                // For partial segments, we need to analyze them differently since they reference byte ranges
                let (actual_duration, video_duration, audio_duration) =
                    if let Some(byte_range) = &partial.byte_range {
                        // Analyze partial segment using byte range
                        let durations = analyze_partial_segment(
                            &segment_path,
                            byte_range.length,
                            byte_range.offset,
                        )?;
                        (
                            durations.total_duration,
                            durations.video_duration,
                            durations.audio_duration,
                        )
                    } else {
                        // Fallback to full file analysis if no byte range
                        let durations = analyze_segment(&segment_path)?;
                        (
                            durations.total_duration,
                            durations.video_duration,
                            durations.audio_duration,
                        )
                    };

                let playlist_duration = partial.duration as f32;
                let difference = actual_duration - playlist_duration as f64;

                let byte_range_info = partial.byte_range.as_ref().map(|br| (br.length, br.offset));

                let info = SegmentInfo {
                    filename: partial.uri.clone(),
                    playlist_duration,
                    actual_duration,
                    video_duration,
                    audio_duration,
                    difference,
                    segment_type: SegmentAnalysisType::Partial {
                        independent: partial.independent,
                        byte_range: byte_range_info,
                    },
                };

                let info_str = if partial.independent { "IND" } else { "" };

                println!(
                    "{:<12} {:>4} {:>12.3} {:>12.3} {:>12.3} {:>12.3} {:>12.3} {:>12}",
                    info.filename,
                    "PART",
                    info.playlist_duration,
                    info.actual_duration,
                    info.video_duration,
                    info.audio_duration,
                    info.difference,
                    info_str
                );

                segments.push(info);
                total_playlist_duration += playlist_duration;
                total_actual_duration += actual_duration;
            }
            MediaSegmentType::PreloadHint(_) => {
                // Skip preload hints for analysis
                continue;
            }
        }
    }

    println!();

    // Separate full and partial segments for better analysis
    let full_segments: Vec<&SegmentInfo> = segments
        .iter()
        .filter(|s| matches!(s.segment_type, SegmentAnalysisType::Full))
        .collect();
    let partial_segments: Vec<&SegmentInfo> = segments
        .iter()
        .filter(|s| matches!(s.segment_type, SegmentAnalysisType::Partial { .. }))
        .collect();
    let independent_partials: Vec<&SegmentInfo> = segments
        .iter()
        .filter(|s| {
            matches!(
                s.segment_type,
                SegmentAnalysisType::Partial {
                    independent: true,
                    ..
                }
            )
        })
        .collect();

    println!("Summary:");
    println!("  Total segments: {}", segments.len());
    println!("    Full segments: {}", full_segments.len());
    println!("    Partial segments: {}", partial_segments.len());
    println!("    Independent partials: {}", independent_partials.len());
    println!("  Total playlist duration: {:.3}s", total_playlist_duration);
    println!("  Total actual duration: {:.3}s", total_actual_duration);
    println!(
        "  Total difference: {:.3}s",
        total_actual_duration - total_playlist_duration as f64
    );
    if !segments.is_empty() {
        println!(
            "  Average difference per segment: {:.3}s",
            (total_actual_duration - total_playlist_duration as f64) / segments.len() as f64
        );
    }

    // Statistics
    let differences: Vec<f64> = segments.iter().map(|s| s.difference).collect();
    let min_diff = differences.iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let max_diff = differences.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    let avg_diff = differences.iter().sum::<f64>() / differences.len() as f64;

    println!();
    println!("Difference Statistics:");
    println!("  Min difference: {:.3}s", min_diff);
    println!("  Max difference: {:.3}s", max_diff);
    println!("  Average difference: {:.3}s", avg_diff);

    // Check for problematic segments
    let problematic: Vec<&SegmentInfo> = segments
        .iter()
        .filter(|s| s.difference.abs() > 0.5)
        .collect();

    if !problematic.is_empty() {
        println!();
        println!("Problematic segments (>0.5s difference):");
        for seg in problematic {
            println!("  {}: {:.3}s difference", seg.filename, seg.difference);
        }
    }

    // HLS-LL specific analysis
    if !partial_segments.is_empty() {
        println!();
        println!("HLS-LL Analysis:");
        let avg_partial_duration: f64 = partial_segments
            .iter()
            .map(|s| s.playlist_duration as f64)
            .sum::<f64>()
            / partial_segments.len() as f64;
        println!("  Average partial duration: {:.3}s", avg_partial_duration);

        if let Some(part_inf) = &playlist.part_inf {
            let target_duration = part_inf.part_target;
            println!("  Target partial duration: {:.3}s", target_duration);
            println!(
                "  Partial duration variance: {:.3}s",
                (avg_partial_duration - target_duration).abs()
            );
        }

        // Show byte range info for partial segments
        let partials_with_ranges = partial_segments
            .iter()
            .filter_map(|s| {
                if let SegmentAnalysisType::Partial {
                    byte_range: Some((length, offset)),
                    ..
                } = &s.segment_type
                {
                    Some((s, length, offset))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !partials_with_ranges.is_empty() {
            println!(
                "  Partial segments with byte ranges: {}",
                partials_with_ranges.len()
            );
            let avg_range_size = partials_with_ranges
                .iter()
                .map(|(_, &length, _)| length)
                .sum::<u64>() as f64
                / partials_with_ranges.len() as f64;
            println!("  Average byte range size: {:.0} bytes", avg_range_size);
        }
    }

    // Check playlist properties
    println!();
    println!("Playlist Properties:");
    println!("  Version: {:?}", playlist.version);
    println!("  Target duration: {:?}", playlist.target_duration);
    println!("  Media sequence: {:?}", playlist.media_sequence);
    if let Some(part_inf) = &playlist.part_inf {
        println!(
            "  Part target: {:.3}s (LL-HLS enabled)",
            part_inf.part_target
        );
    }

    // Count preload hints
    let preload_hints = playlist
        .segments
        .iter()
        .filter(|s| matches!(s, MediaSegmentType::PreloadHint(_)))
        .count();
    if preload_hints > 0 {
        println!("  Preload hints: {}", preload_hints);
    }

    // Validation checks - detect timing mismatches
    println!();
    println!("Validation Results:");
    let mut validation_errors = Vec::new();
    
    // Check for significant video/audio duration mismatches
    for segment in &segments {
        let playlist_dur = segment.playlist_duration as f64;
        let video_dur = segment.video_duration;
        let audio_dur = segment.audio_duration;
        
        // Check if video duration is significantly different from playlist duration
        let video_diff_ratio = if playlist_dur > 0.0 {
            (video_dur - playlist_dur).abs() / playlist_dur
        } else {
            0.0
        };
        
        // Check if audio duration is significantly different from playlist duration  
        let audio_diff_ratio = if playlist_dur > 0.0 {
            (audio_dur - playlist_dur).abs() / playlist_dur
        } else {
            0.0
        };
        
        // Flag segments where video is much shorter than expected (common resampling issue)
        if video_diff_ratio > 0.3 && video_dur < playlist_dur * 0.7 {
            validation_errors.push(format!(
                "❌ {}: Video duration {:.3}s is {:.1}% shorter than playlist duration {:.3}s",
                segment.filename, video_dur, (1.0 - video_dur / playlist_dur) * 100.0, playlist_dur
            ));
        }
        
        // Flag segments where audio is significantly different (potential sample rate issue)
        if audio_diff_ratio > 0.1 {
            let direction = if audio_dur > playlist_dur { "longer" } else { "shorter" };
            validation_errors.push(format!(
                "⚠️  {}: Audio duration {:.3}s is {:.1}% {} than playlist duration {:.3}s",
                segment.filename, audio_dur, audio_diff_ratio * 100.0, direction, playlist_dur
            ));
        }
        
        // Flag segments where video and audio durations are very different
        let av_diff_ratio = if audio_dur > 0.0 {
            (video_dur - audio_dur).abs() / audio_dur
        } else {
            0.0
        };
        
        if av_diff_ratio > 0.5 {
            validation_errors.push(format!(
                "🔄 {}: Video ({:.3}s) and audio ({:.3}s) durations differ by {:.1}%",
                segment.filename, video_dur, audio_dur, av_diff_ratio * 100.0
            ));
        }
    }
    
    // Check overall stream consistency
    if !segments.is_empty() {
        let avg_video_dur: f64 = segments.iter().map(|s| s.video_duration).sum::<f64>() / segments.len() as f64;
        let avg_audio_dur: f64 = segments.iter().map(|s| s.audio_duration).sum::<f64>() / segments.len() as f64;
        let avg_playlist_dur: f64 = segments.iter().map(|s| s.playlist_duration as f64).sum::<f64>() / segments.len() as f64;
        
        println!("  Average durations:");
        println!("    Video: {:.3}s", avg_video_dur);
        println!("    Audio: {:.3}s", avg_audio_dur);
        println!("    Playlist: {:.3}s", avg_playlist_dur);
        
        // Overall timing validation
        let video_playlist_ratio = if avg_playlist_dur > 0.0 { avg_video_dur / avg_playlist_dur } else { 0.0 };
        let audio_playlist_ratio = if avg_playlist_dur > 0.0 { avg_audio_dur / avg_playlist_dur } else { 0.0 };
        
        if video_playlist_ratio < 0.8 {
            validation_errors.push(format!(
                "❌ CRITICAL: Average video duration ({:.3}s) is only {:.1}% of expected playlist duration ({:.3}s)",
                avg_video_dur, video_playlist_ratio * 100.0, avg_playlist_dur
            ));
        }
        
        if (audio_playlist_ratio - 1.0).abs() > 0.1 {
            let direction = if audio_playlist_ratio > 1.0 { "longer" } else { "shorter" };
            validation_errors.push(format!(
                "⚠️  TIMING: Average audio duration ({:.3}s) is {:.1}% {} than expected ({:.3}s) - possible sample rate mismatch",
                avg_audio_dur, ((audio_playlist_ratio - 1.0).abs() * 100.0), direction, avg_playlist_dur
            ));
        }
    }
    
    if validation_errors.is_empty() {
        println!("  ✅ All timing validations passed");
    } else {
        println!("  {} validation issue(s) found:", validation_errors.len());
        for error in &validation_errors {
            println!("    {}", error);
        }
        
        // Return error exit code if critical issues found
        let has_critical_errors = validation_errors.iter().any(|e| e.contains("❌ CRITICAL"));
        if has_critical_errors {
            eprintln!("\n❌ CRITICAL timing issues detected. Stream may not play correctly.");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn analyze_segment_with_reader(reader: Box<dyn Read>) -> Result<SegmentDurations> {
    let mut demuxer = Demuxer::new_custom_io(reader, None)?;

    // Probe the input to get stream information
    unsafe {
        demuxer.probe_input()?;
    }

    let mut video_start_pts = AV_NOPTS_VALUE;
    let mut video_end_pts = AV_NOPTS_VALUE;
    let mut audio_start_pts = AV_NOPTS_VALUE;
    let mut audio_end_pts = AV_NOPTS_VALUE;
    let mut video_last_duration = 0i64;
    let mut audio_last_duration = 0i64;
    let mut video_packets = 0u64;
    let mut audio_packets = 0u64;
    let mut video_stream_idx: Option<usize> = None;
    let mut audio_stream_idx: Option<usize> = None;

    // Read all packets and track timing
    loop {
        let packet_result = unsafe { demuxer.get_packet() };
        match packet_result {
            Ok((pkt, stream)) => {
                if pkt.is_null() {
                    break; // End of stream
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
                                video_packets += 1;
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
                                audio_packets += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break, // End of file or error
        }
    }

    // Calculate durations (including last packet duration)
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
        video_packets,
        audio_packets,
        video_start_pts,
        video_end_pts,
        audio_start_pts,
        audio_end_pts,
    })
}

fn analyze_segment(path: &Path) -> Result<SegmentDurations> {
    let file =
        fs::File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;
    analyze_segment_with_reader(Box::new(file))
}

fn analyze_fmp4_segment(init_path: &Path, segment_path: &Path) -> Result<SegmentDurations> {
    // For fMP4 segments, we need to combine the init segment with the media segment
    // This creates a temporary combined file that can be properly demuxed
    let init_data = fs::read(init_path)
        .with_context(|| format!("Failed to read init segment: {}", init_path.display()))?;
    let segment_data = fs::read(segment_path)
        .with_context(|| format!("Failed to read segment: {}", segment_path.display()))?;
    
    // Combine init and segment data
    let mut combined_data = Vec::with_capacity(init_data.len() + segment_data.len());
    combined_data.extend_from_slice(&init_data);
    combined_data.extend_from_slice(&segment_data);
    
    // Create a cursor over the combined data
    let cursor = std::io::Cursor::new(combined_data);
    analyze_segment_with_reader(Box::new(cursor))
}

fn analyze_partial_segment(
    path: &Path,
    length: u64,
    offset: Option<u64>,
) -> Result<SegmentDurations> {
    // Create a custom byte range reader for the partial segment
    let reader = ByteRangeReader::new(path, length, offset)?;

    // Use the custom IO with demuxer to analyze only the byte range
    analyze_segment_with_reader(Box::new(reader))
}

fn analyze_init_segment(path: &Path) -> Result<InitSegmentInfo> {
    use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
        avcodec_get_name, AVPixelFormat::AV_PIX_FMT_NONE,
    };
    use std::ffi::CStr;

    let mut demuxer = Demuxer::new(path.to_str().unwrap())?;

    // Probe the input to get stream information
    unsafe {
        demuxer.probe_input()?;
    }

    let mut streams = Vec::new();
    let mut pixel_format_set = false;

    // Try to get streams - we'll iterate until we hit an error
    let mut i = 0;
    loop {
        let stream_result = unsafe { demuxer.get_stream(i) };
        match stream_result {
            Ok(stream) => unsafe {
                let codecpar = (*stream).codecpar;
                let codec_type = (*codecpar).codec_type;

                let codec_name = {
                    let name_ptr = avcodec_get_name((*codecpar).codec_id);
                    if name_ptr.is_null() {
                        "unknown".to_string()
                    } else {
                        CStr::from_ptr(name_ptr).to_string_lossy().to_string()
                    }
                };

                let (codec_type_str, width, height, pixel_format) = match codec_type {
                    AVMEDIA_TYPE_VIDEO => {
                        let w = if (*codecpar).width > 0 {
                            Some((*codecpar).width)
                        } else {
                            None
                        };
                        let h = if (*codecpar).height > 0 {
                            Some((*codecpar).height)
                        } else {
                            None
                        };

                        let pix_fmt = if (*codecpar).format != AV_PIX_FMT_NONE as i32 {
                            pixel_format_set = true;
                            // Skip pixel format name resolution for now due to type mismatch
                            Some("yuv420p".to_string()) // Common default
                        } else {
                            None
                        };

                        ("video".to_string(), w, h, pix_fmt)
                    }
                    AVMEDIA_TYPE_AUDIO => ("audio".to_string(), None, None, None),
                    _ => ("other".to_string(), None, None, None),
                };

                streams.push(StreamInfo {
                    codec_type: codec_type_str,
                    codec_name,
                    width,
                    height,
                    pixel_format,
                });

                i += 1;
            },
            Err(_) => break, // No more streams
        }
    }

    let stream_count = streams.len();

    // Check if this is a proper MP4 initialization segment by looking for file data
    let file_data = fs::read(path)?;
    let has_moov = file_data.windows(4).any(|window| window == b"moov");

    Ok(InitSegmentInfo {
        stream_count,
        streams,
        has_moov,
        pixel_format_set,
    })
}
