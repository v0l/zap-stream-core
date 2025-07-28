use crate::egress::{EgressResult, EgressSegment, EncoderOrSourceStream};
use crate::hash_file_sync;
use crate::mux::hls::segment::{HlsSegment, PartialSegmentInfo, SegmentInfo};
use crate::mux::{HlsVariantStream, SegmentType};
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{bail, ensure, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVMediaType::AVMEDIA_TYPE_VIDEO;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_dump_format, av_free, av_get_bits_per_pixel, av_pix_fmt_desc_get, av_q2d, av_write_frame,
    avio_close, avio_flush, avio_open, avio_size, AVPacket, AVIO_FLAG_WRITE, AV_NOPTS_VALUE,
    AV_PKT_FLAG_KEY,
};
use ffmpeg_rs_raw::{cstr, Muxer};
use log::{debug, info, trace, warn};
use m3u8_rs::{ExtTag, MediaSegmentType, PartInf, PreloadHint};
use std::collections::HashMap;
use std::fs::{create_dir_all, File};
use std::mem::transmute;
use std::path::PathBuf;
use std::ptr;

pub struct HlsVariant {
    /// Name of this variant (720p)
    name: String,
    /// MPEG-TS muxer for this variant
    mux: Muxer,
    /// List of streams ids in this variant
    pub(crate) streams: Vec<HlsVariantStream>,
    /// Segment length in seconds
    segment_length_target: f32,
    /// Total number of seconds of video to store
    segment_window: f32,
    /// Current segment index
    idx: u64,
    /// Output directory (base)
    out_dir: PathBuf,
    /// List of segments to be included in the playlist
    pub(crate) segments: Vec<HlsSegment>,
    /// Type of segments to create
    pub(crate) segment_type: SegmentType,
    /// Timestamp of the start of the current segment
    current_segment_start: f64,
    /// Timestamp of the start of the current partial
    current_partial_start: f64,
    /// Number of packets written to current segment
    packets_written: u64,
    /// Reference stream used to track duration
    ref_stream_index: i32,
    /// HLS-LL: Enable LL-output
    low_latency: bool,
    /// LL-HLS: Target duration for partial segments
    partial_target_duration: f32,
    /// HLS-LL: Current partial index
    current_partial_index: u64,
    /// HLS-LL: Whether the next partial segment should be marked as independent
    next_partial_independent: bool,
    /// Path to initialization segment for fMP4
    init_segment_path: Option<String>,
}

impl HlsVariant {
    pub fn new<'a>(
        out_dir: PathBuf,
        group: usize,
        encoded_vars: impl Iterator<Item = (&'a VariantStream, EncoderOrSourceStream<'a>)>,
        segment_type: SegmentType,
        mut segment_length: f32,
    ) -> Result<Self> {
        let name = format!("stream_{}", group);

        let var_dir = out_dir.join(&name);
        if !var_dir.exists() {
            create_dir_all(&var_dir)?;
        }

        let mut mux = unsafe {
            Muxer::builder()
                .with_output_path(
                    var_dir
                        .join(match segment_type {
                            SegmentType::MPEGTS => "1.ts",
                            SegmentType::FMP4 => "1.m4s",
                        })
                        .to_str()
                        .unwrap(),
                    match segment_type {
                        SegmentType::MPEGTS => Some("mpegts"),
                        SegmentType::FMP4 => Some("mp4"),
                    },
                )?
                .build()?
        };
        let mut streams = Vec::new();
        let mut ref_stream_index = -1;
        let mut has_video = false;

        for (var, enc) in encoded_vars {
            match enc {
                EncoderOrSourceStream::Encoder(enc) => match var {
                    VariantStream::Video(v) => unsafe {
                        let stream = mux.add_stream_encoder(enc)?;
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Video {
                            group,
                            index: stream_idx,
                            id: v.id(),
                        });
                        has_video = true;
                        ref_stream_index = stream_idx as _;
                        let sg = v.keyframe_interval as f32 / v.fps;
                        if sg > segment_length {
                            segment_length = sg;
                        }
                    },
                    VariantStream::Audio(a) => unsafe {
                        let stream = mux.add_stream_encoder(enc)?;
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Audio {
                            group,
                            index: stream_idx,
                            id: a.id(),
                        });
                        if !has_video && ref_stream_index == -1 {
                            ref_stream_index = stream_idx as _;
                        }
                    },
                    VariantStream::Subtitle(s) => unsafe {
                        let stream = mux.add_stream_encoder(enc)?;
                        streams.push(HlsVariantStream::Subtitle {
                            group,
                            index: (*stream).index as usize,
                            id: s.id(),
                        })
                    },
                    _ => bail!("unsupported variant stream"),
                },
                EncoderOrSourceStream::SourceStream(stream) => match var {
                    VariantStream::CopyVideo(v) => unsafe {
                        let stream = mux.add_copy_stream(stream)?;
                        (*(*stream).codecpar).codec_tag = 0; // fix copy tag
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Video {
                            group,
                            index: stream_idx,
                            id: v.id(),
                        });
                        has_video = true;
                        ref_stream_index = stream_idx as _;
                    },
                    VariantStream::CopyAudio(a) => unsafe {
                        let stream = mux.add_copy_stream(stream)?;
                        (*(*stream).codecpar).codec_tag = 0; // fix copy tag
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Audio {
                            group,
                            index: stream_idx,
                            id: a.id(),
                        });
                        if !has_video && ref_stream_index == -1 {
                            ref_stream_index = stream_idx as _;
                        }
                    },
                    _ => bail!("unsupported variant stream"),
                },
            }
        }
        ensure!(
            ref_stream_index != -1,
            "No reference stream found, cant create variant"
        );
        info!(
            "{} will use stream index {} as reference for segmentation",
            name, ref_stream_index
        );

        let mut opts = HashMap::new();
        if let SegmentType::FMP4 = segment_type {
            // Proper fMP4 segmentation flags for HLS
            opts.insert(
                "movflags".to_string(),
                "+frag_keyframe+empty_moov+default_base_moof".to_string(),
            );
        };

        unsafe {
            mux.open(Some(opts))?;
            //av_dump_format(mux.context(), 0, ptr::null_mut(), 0);
        }

        let variant = Self {
            name: name.clone(),
            segment_window: 30.0,
            mux,
            streams,
            idx: 1,
            segments: Vec::new(),
            out_dir: var_dir,
            segment_type,
            current_segment_start: 0.0,
            current_partial_start: 0.0,
            packets_written: 0,
            ref_stream_index,
            low_latency: false,
            partial_target_duration: 0.0,
            current_partial_index: 0,
            next_partial_independent: false,
            segment_length_target: segment_length,
            init_segment_path: None,
        };

        Ok(variant)
    }

    pub fn segment_length(&self) -> f32 {
        let min_segment_length = if self.low_latency {
            (self.segment_length_target * 3.0).max(6.0) // make segments 3x longer in LL mode or minimum 6s
        } else {
            2.0
        };
        self.segment_length_target.max(min_segment_length)
    }

    pub fn partial_segment_length(&self) -> f32 {
        let seg_size = self.segment_length();
        let partial_seg_size = seg_size / 3.0; // 3 segments min
        partial_seg_size - partial_seg_size % seg_size
    }

    pub fn segment_name(t: SegmentType, idx: u64) -> String {
        match t {
            SegmentType::MPEGTS => format!("{}.ts", idx),
            SegmentType::FMP4 => format!("{}.m4s", idx),
        }
    }

    pub fn map_segment_path(&self, idx: u64, typ: SegmentType) -> PathBuf {
        self.out_dir.join(Self::segment_name(typ, idx))
    }

    /// Process a single packet through the muxer
    pub(crate) unsafe fn process_packet(&mut self, pkt: *mut AVPacket) -> Result<EgressResult> {
        let pkt_stream = *(*self.mux.context())
            .streams
            .add((*pkt).stream_index as usize);

        let pkt_q = av_q2d((*pkt).time_base);
        let mut result = EgressResult::None;
        let stream_type = (*(*pkt_stream).codecpar).codec_type;
        let mut can_split = stream_type == AVMEDIA_TYPE_VIDEO
            && ((*pkt).flags & AV_PKT_FLAG_KEY == AV_PKT_FLAG_KEY);
        let mut is_ref_pkt = (*pkt).stream_index == self.ref_stream_index;

        if (*pkt).pts == AV_NOPTS_VALUE {
            can_split = false;
            is_ref_pkt = false;
        }

        if is_ref_pkt {
            let pkt_duration = (*pkt).duration as f64 * pkt_q;
            trace!(
                "REF PKT index={}, pts={:.3}s, dur={:.3}s, flags={}",
                (*pkt).stream_index,
                (*pkt).pts as f64 * pkt_q,
                pkt_duration,
                (*pkt).flags
            );
        }
        if is_ref_pkt && self.packets_written > 0 {
            let pkt_pts = (*pkt).pts as f64 * pkt_q;
            let cur_duration = pkt_pts - self.current_segment_start;
            let cur_part_duration = pkt_pts - self.current_partial_start;

            // check if current packet is keyframe, flush current segment
            if can_split && cur_duration >= self.segment_length() as f64 {
                result = self.split_next_seg(pkt_pts)?;
            } else if self.low_latency && cur_part_duration >= self.partial_target_duration as f64 {
                result = self.create_partial_segment(pkt_pts)?;
                self.next_partial_independent = can_split;
            }
        }

        // write to current segment
        self.mux.write_packet(pkt)?;
        self.packets_written += 1;

        Ok(result)
    }

    pub unsafe fn reset(&mut self) -> Result<()> {
        self.mux.close()
    }

    /// Create a partial segment for LL-HLS
    fn create_partial_segment(&mut self, next_pkt_start: f64) -> Result<EgressResult> {
        let ctx = self.mux.context();
        let end_pos = unsafe {
            avio_flush((*ctx).pb);
            avio_size((*ctx).pb) as u64
        };

        ensure!(end_pos > 0, "End position cannot be 0");
        if self.segment_type == SegmentType::MPEGTS {
            ensure!(
                end_pos % 188 == 0,
                "Invalid end position, must be multiple of 188"
            );
        }

        let previous_end_pos = self
            .segments
            .last()
            .and_then(|s| match &s {
                HlsSegment::Partial(p) => p.end_pos(),
                _ => None,
            })
            .unwrap_or(0);
        let partial_size = end_pos - previous_end_pos;
        let partial_info = PartialSegmentInfo {
            index: self.current_partial_index,
            parent_index: self.idx,
            parent_kind: self.segment_type,
            duration: next_pkt_start - self.current_partial_start,
            independent: self.next_partial_independent,
            byte_range: Some((partial_size, Some(previous_end_pos))),
        };

        debug!(
            "{} created partial segment {} [{:.3}s, independent={}]",
            self.name, partial_info.index, partial_info.duration, partial_info.independent,
        );
        self.segments.push(HlsSegment::Partial(partial_info));
        self.current_partial_index += 1;
        self.next_partial_independent = false;
        self.current_partial_start = next_pkt_start;

        self.write_playlist()?;

        Ok(EgressResult::None)
    }

    /// Create initialization segment for fMP4
    unsafe fn create_init_segment(&mut self) -> Result<()> {
        if self.segment_type != SegmentType::FMP4 || self.init_segment_path.is_some() {
            return Ok(());
        }

        let init_path = self.out_dir.join("init.mp4").to_string_lossy().to_string();

        // Create a temporary muxer for initialization segment
        let mut init_opts = HashMap::new();
        init_opts.insert(
            "movflags".to_string(),
            "+frag_keyframe+empty_moov+omit_tfhd_offset+separate_moof+default_base_moof"
                .to_string(),
        );

        let mut init_mux = Muxer::builder()
            .with_output_path(init_path.as_str(), Some("mp4"))?
            .build()?;

        // Copy stream parameters from main muxer
        let main_ctx = self.mux.context();
        for i in 0..(*main_ctx).nb_streams {
            let src_stream = *(*main_ctx).streams.add(i as usize);
            let s = init_mux.add_copy_stream(src_stream)?;
            ensure!((*s).index == (*src_stream).index, "Stream index mismatch");
        }

        init_mux.open(Some(init_opts))?;
        av_write_frame(init_mux.context(), ptr::null_mut());
        init_mux.close()?;

        self.init_segment_path = Some("init.mp4".to_string());
        info!("Created fMP4 initialization segment: {}", init_path);

        Ok(())
    }

    /// Reset the muxer state and start the next segment
    unsafe fn split_next_seg(&mut self, next_pkt_start: f64) -> Result<EgressResult> {
        let completed_segment_idx = self.idx;
        self.idx += 1;
        self.current_partial_index = 0;

        // Create initialization segment after first segment completion
        // This ensures the init segment has the correct timebase from the encoder
        if self.segment_type == SegmentType::FMP4 && self.init_segment_path.is_none() && completed_segment_idx == 1 {
            self.create_init_segment()?;
        }

        // Manually reset muxer avio
        let ctx = self.mux.context();
        let ret = av_write_frame(ctx, ptr::null_mut());
        if ret < 0 {
            bail!("Failed to split segment {}", ret);
        }
        avio_flush((*ctx).pb);
        avio_close((*ctx).pb);
        av_free((*ctx).url as *mut _);

        let next_seg_url = self.map_segment_path(self.idx, self.segment_type);
        (*ctx).url = cstr!(next_seg_url.to_str().unwrap());

        let ret = avio_open(&mut (*ctx).pb, (*ctx).url, AVIO_FLAG_WRITE);
        if ret < 0 {
            bail!("Failed to re-init avio");
        }

        // Log the completed segment (previous index), not the next one
        let completed_seg_path = self.map_segment_path(completed_segment_idx, self.segment_type);
        let segment_size = completed_seg_path.metadata().map(|m| m.len()).unwrap_or(0);

        let cur_duration = next_pkt_start - self.current_segment_start;
        debug!(
            "Finished segment {} [{:.3}s, {:.2} kB, {} pkts]",
            completed_seg_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            cur_duration,
            segment_size as f32 / 1024f32,
            self.packets_written
        );

        let video_var_id = self
            .video_stream()
            .unwrap_or(self.streams.first().unwrap())
            .id()
            .clone();

        // cleanup old segments
        let deleted = self
            .clean_segments()?
            .into_iter()
            .map(|seg| EgressSegment {
                variant: video_var_id,
                idx: seg.index,
                duration: seg.duration,
                path: self.map_segment_path(seg.index, self.segment_type),
                sha256: seg.sha256,
            })
            .collect();

        let hash = {
            let mut f = File::open(&completed_seg_path)?;
            hash_file_sync(&mut f)
        }?;
        // emit result of the previously completed segment,
        let created = EgressSegment {
            variant: video_var_id,
            idx: completed_segment_idx,
            duration: cur_duration as f32,
            path: completed_seg_path,
            sha256: hash,
        };

        self.segments.push(HlsSegment::Full(SegmentInfo {
            index: completed_segment_idx,
            duration: cur_duration as f32,
            kind: self.segment_type,
            sha256: hash,
        }));

        self.write_playlist()?;

        // Reset counters for next segment
        self.packets_written = 0;
        self.current_segment_start = next_pkt_start;

        Ok(EgressResult::Segments {
            created: vec![created],
            deleted,
        })
    }

    pub fn video_stream(&self) -> Option<&HlsVariantStream> {
        self.streams
            .iter()
            .find(|a| matches!(*a, HlsVariantStream::Video { .. }))
    }

    /// Delete segments which are too old
    fn clean_segments(&mut self) -> Result<Vec<SegmentInfo>> {
        let drain_from_hls_segment = {
            let mut acc = 0.0;
            let mut seg_match = None;
            for seg in self
                .segments
                .iter()
                .filter(|e| matches!(e, HlsSegment::Full(_)))
                .rev()
            {
                if acc >= self.segment_window {
                    seg_match = Some(seg);
                    break;
                }
                acc += match seg {
                    HlsSegment::Full(seg) => seg.duration,
                    _ => 0.0,
                };
            }
            seg_match
        };
        let mut ret = vec![];
        if let Some(seg_match) = drain_from_hls_segment {
            if let Some(drain_pos) = self.segments.iter().position(|e| e == seg_match) {
                for seg in self.segments.drain(..drain_pos) {
                    match seg {
                        HlsSegment::Full(seg) => {
                            let seg_path = self.out_dir.join(seg.filename());
                            if let Err(e) = std::fs::remove_file(&seg_path) {
                                warn!(
                                    "Failed to remove segment file: {} {}",
                                    seg_path.display(),
                                    e
                                );
                            }
                            trace!("Removed segment file: {}", seg_path.display());

                            ret.push(seg);
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(ret)
    }

    fn playlist_version(&self) -> usize {
        if self.low_latency {
            6
        } else if self.segment_type == SegmentType::FMP4 {
            6 // EXT-X-MAP without I-FRAMES-ONLY
        } else {
            3
        }
    }

    fn write_playlist(&mut self) -> Result<()> {
        if self.segments.is_empty() {
            return Ok(()); // Don't write empty playlists
        }

        let mut pl = m3u8_rs::MediaPlaylist::default();
        pl.segments = self.segments.iter().map(|s| s.to_media_segment()).collect();

        // Add EXT-X-MAP initialization segment for fMP4
        if self.segment_type == SegmentType::FMP4 {
            if let Some(ref init_path) = self.init_segment_path {
                pl.unknown_tags.push(ExtTag {
                    tag: "X-MAP".to_string(),
                    rest: Some(format!("URI=\"{}\"", init_path)),
                });
            }
        }

        // append segment preload for next part segment
        if let Some(HlsSegment::Partial(partial)) = self.segments.last() {
            // TODO: try to estimate if there will be another partial segment
            pl.segments.push(MediaSegmentType::PreloadHint(PreloadHint {
                hint_type: "PART".to_string(),
                uri: partial.filename(),
                byte_range_start: partial.end_pos(),
                byte_range_length: None,
            }));
        }

        pl.version = Some(self.playlist_version());
        pl.target_duration = if self.playlist_version() >= 6 {
            self.segment_length().round() as _
        } else {
            self.segment_length()
        };
        if self.low_latency {
            pl.part_inf = Some(PartInf {
                part_target: self.partial_target_duration as f64,
            });
        }
        pl.media_sequence = self
            .segments
            .iter()
            .find_map(|s| match s {
                HlsSegment::Full(ss) => Some(ss.index),
                _ => None,
            })
            .unwrap_or(self.idx);
        pl.end_list = false;

        let mut f_out = File::create(self.out_dir.join("live.m3u8"))?;
        pl.write_to(&mut f_out)?;
        Ok(())
    }

    unsafe fn to_codec_attr(&self) -> Option<String> {
        let mut codecs = Vec::new();

        // Find video and audio streams and build codec string
        for stream in &self.streams {
            let av_stream = *(*self.mux.context()).streams.add(*stream.index());
            let p = (*av_stream).codecpar;

            match stream {
                HlsVariantStream::Video { .. } => {
                    if (*p).codec_id == AV_CODEC_ID_H264 {
                        // Use profile and level from codec parameters
                        let profile_idc = (*p).profile as u8;
                        let level_idc = (*p).level as u8;

                        // For H.264, constraint flags are typically 0 unless specified
                        // Common constraint flags: 0x40 (constraint_set1_flag) for baseline
                        let constraint_flags = match profile_idc {
                            66 => 0x40, // Baseline profile
                            _ => 0x00,  // Main/High profiles typically have no constraints
                        };

                        let avc1_code = format!(
                            "avc1.{:02x}{:02x}{:02x}",
                            profile_idc, constraint_flags, level_idc
                        );
                        codecs.push(avc1_code);
                    }
                }
                HlsVariantStream::Audio { .. } => {
                    // Standard AAC-LC codec string
                    codecs.push("mp4a.40.2".to_string());
                }
                _ => {}
            }
        }

        if codecs.is_empty() {
            None
        } else {
            Some(codecs.join(","))
        }
    }

    pub fn to_playlist_variant(&self) -> m3u8_rs::VariantStream {
        unsafe {
            let pes = self.video_stream().unwrap_or(self.streams.first().unwrap());
            let av_stream = *(*self.mux.context()).streams.add(*pes.index());
            let codec_par = (*av_stream).codecpar;
            let bitrate = (*codec_par).bit_rate as u64;
            let fps = av_q2d((*codec_par).framerate);
            m3u8_rs::VariantStream {
                is_i_frame: false,
                uri: format!("{}/live.m3u8", self.name),
                bandwidth: if bitrate == 0 {
                    // make up bitrate when unknown (copy streams)
                    // this is the bitrate as a raw decoded stream, it's not accurate at all
                    // It only serves the purpose of ordering the copy streams as having the highest bitrate
                    let pix_desc = av_pix_fmt_desc_get(transmute((*codec_par).format));
                    (*codec_par).width as u64
                        * (*codec_par).height as u64
                        * av_get_bits_per_pixel(pix_desc) as u64
                } else {
                    bitrate
                },
                average_bandwidth: None,
                codecs: self.to_codec_attr(),
                resolution: Some(m3u8_rs::Resolution {
                    width: (*codec_par).width as _,
                    height: (*codec_par).height as _,
                }),
                frame_rate: if fps > 0.0 { Some(fps) } else { None },
                hdcp_level: None,
                audio: None,
                video: None,
                subtitles: None,
                closed_captions: None,
                other_attributes: None,
            }
        }
    }
}
