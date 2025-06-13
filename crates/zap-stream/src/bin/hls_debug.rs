use anyhow::{Context, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_q2d, AV_NOPTS_VALUE, AVMediaType::AVMEDIA_TYPE_VIDEO, AVMediaType::AVMEDIA_TYPE_AUDIO,
};
use ffmpeg_rs_raw::Demuxer;
use m3u8_rs::{parse_media_playlist, MediaSegmentType};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct SegmentInfo {
    filename: String,
    playlist_duration: f32,
    actual_duration: f64,
    video_duration: f64,
    audio_duration: f64,
    difference: f64,
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

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <path_to_hls_directory>", args[0]);
        eprintln!("Example: {} out/hls/8c220348-fdbb-44cd-94d5-97a11a9ec91d/stream_0", args[0]);
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
    println!();

    // Parse the playlist
    let playlist_content = fs::read_to_string(&playlist_path)
        .context("Failed to read playlist file")?;
    
    let (_, playlist) = parse_media_playlist(playlist_content.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to parse playlist: {:?}", e))?;

    // Analyze each segment
    let mut segments = Vec::new();
    let mut total_playlist_duration = 0.0f32;
    let mut total_actual_duration = 0.0f64;

    println!("Segment Analysis:");
    println!("{:<12} {:>12} {:>12} {:>12} {:>12} {:>12}", 
             "Segment", "Playlist", "Actual", "Video", "Audio", "Difference");
    println!("{:<12} {:>12} {:>12} {:>12} {:>12} {:>12}", 
             "--------", "--------", "------", "-----", "-----", "----------");

    for segment_type in &playlist.segments {
        if let MediaSegmentType::Full(segment) = segment_type {
            let segment_path = hls_dir.join(&segment.uri);
            
            if !segment_path.exists() {
                eprintln!("Warning: Segment file {:?} does not exist", segment_path);
                continue;
            }

            // Analyze file using demuxer
            let durations = analyze_segment(&segment_path)?;
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
            };

            println!("{:<12} {:>12.3} {:>12.3} {:>12.3} {:>12.3} {:>12.3}", 
                     info.filename, 
                     info.playlist_duration, 
                     info.actual_duration, 
                     info.video_duration, 
                     info.audio_duration, 
                     info.difference);

            segments.push(info);
            total_playlist_duration += playlist_duration;
            total_actual_duration += actual_duration;
        }
    }

    println!();
    println!("Summary:");
    println!("  Total segments: {}", segments.len());
    println!("  Total playlist duration: {:.3}s", total_playlist_duration);
    println!("  Total actual duration: {:.3}s", total_actual_duration);
    println!("  Total difference: {:.3}s", total_actual_duration - total_playlist_duration as f64);
    println!("  Average difference per segment: {:.3}s", 
             (total_actual_duration - total_playlist_duration as f64) / segments.len() as f64);

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
    let problematic: Vec<&SegmentInfo> = segments.iter()
        .filter(|s| s.difference.abs() > 0.5)
        .collect();

    if !problematic.is_empty() {
        println!();
        println!("Problematic segments (>0.5s difference):");
        for seg in problematic {
            println!("  {}: {:.3}s difference", seg.filename, seg.difference);
        }
    }

    // Check playlist properties
    println!();
    println!("Playlist Properties:");
    println!("  Version: {:?}", playlist.version);
    println!("  Target duration: {:?}", playlist.target_duration);
    println!("  Media sequence: {:?}", playlist.media_sequence);
    if let Some(part_inf) = &playlist.part_inf {
        println!("  Part target: {:.3}s (LL-HLS enabled)", part_inf.part_target);
    }

    Ok(())
}

fn analyze_segment(path: &Path) -> Result<SegmentDurations> {
    let mut demuxer = Demuxer::new(path.to_str().unwrap())?;
    
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