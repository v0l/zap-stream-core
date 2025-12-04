use crate::egress::{EgressResult, EgressSegment, EncoderOrSourceStream, EncoderVariantGroup};
use crate::hash_file_sync;
use crate::mux::hls::segment::{HlsSegment, PartialSegmentInfo, SegmentInfo};
use crate::mux::{HlsVariantStream, SegmentType};
use crate::variant::VariantStream;
use anyhow::{Result, bail, ensure};
use chrono::Utc;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVMediaType::AVMEDIA_TYPE_VIDEO;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AV_NOPTS_VALUE, AV_PKT_FLAG_KEY, AVIO_FLAG_WRITE, av_freep, av_get_bits_per_pixel,
    av_interleaved_write_frame, av_pix_fmt_desc_get, av_q2d, av_write_frame, avio_closep,
    avio_flush, avio_open, avio_size,
};
use ffmpeg_rs_raw::{AvPacketRef, Muxer, bail_ffmpeg, cstr};
use m3u8_rs::Playlist::MediaPlaylist;
use m3u8_rs::{ExtTag, MediaSegmentType, PartInf, Playlist, PreloadHint};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{File, create_dir_all};
use std::mem::transmute;
use std::path::PathBuf;
use std::ptr;
use tracing::{debug, error, info, trace, warn};

pub struct HlsVariant {
    /// Name of this variant (720p)
    name: String,
    /// MPEG-TS muxer for this variant
    mux: Muxer,
    /// List of streams ids in this variant
    pub(crate) streams: Vec<HlsVariantStream>,
    /// Segment length in seconds
    pub(crate) segment_length_target: f32,
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
    pub const PLAYLIST_NAME: &'static str = "live.m3u8";

    pub fn new(
        out_dir: PathBuf,
        group: &EncoderVariantGroup,
        segment_type: SegmentType,
        mut segment_length: f32,
    ) -> Result<Self> {
        let name = group.id.to_string();

        let mut segments = Vec::new();
        let var_dir = out_dir.join(&name);
        if !var_dir.exists() {
            create_dir_all(&var_dir)?;
        } else {
            // resume seq, read playlist, avoid CDN cache hits for previous stream
            match Self::try_load_media_seq(&var_dir) {
                Ok(i) => {
                    // setup segments
                    segments = i;
                    // mark last segment as discontinuity
                    if let Some(HlsSegment::Full(last)) = segments
                        .iter_mut()
                        .rfind(|s| matches!(s, HlsSegment::Full(_)))
                    {
                        last.discontinuity = true;
                    }
                }
                Err(e) => {
                    warn!("Failed to load media sequence: {}", e);
                }
            }
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

        for g in &group.streams {
            match g.stream {
                EncoderOrSourceStream::Encoder(enc) => match g.variant {
                    VariantStream::Video(v) => unsafe {
                        let stream = mux.add_stream_encoder(enc)?;
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Video {
                            index: stream_idx,
                            id: v.id,
                        });
                        has_video = true;
                        ref_stream_index = stream_idx as _;
                        let sg = v.gop as f32 / v.fps;
                        if sg > segment_length {
                            segment_length = sg;
                        }
                    },
                    VariantStream::Audio(a) => unsafe {
                        let stream = mux.add_stream_encoder(enc)?;
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Audio {
                            index: stream_idx,
                            id: a.id,
                        });
                        if !has_video && ref_stream_index == -1 {
                            ref_stream_index = stream_idx as _;
                        }
                    },
                    VariantStream::Subtitle { id, .. } => unsafe {
                        let stream = mux.add_stream_encoder(enc)?;
                        streams.push(HlsVariantStream::Subtitle {
                            index: (*stream).index as usize,
                            id: *id,
                        })
                    },
                    _ => bail!("unsupported variant stream"),
                },
                EncoderOrSourceStream::SourceStream(stream) => match g.variant {
                    VariantStream::CopyVideo(v) => unsafe {
                        let stream = mux.add_copy_stream(stream)?;
                        (*(*stream).codecpar).codec_tag = 0; // fix copy tag
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Video {
                            index: stream_idx,
                            id: v.id,
                        });
                        has_video = true;
                        ref_stream_index = stream_idx as _;
                    },
                    VariantStream::CopyAudio(a) => unsafe {
                        let stream = mux.add_copy_stream(stream)?;
                        (*(*stream).codecpar).codec_tag = 0; // fix copy tag
                        let stream_idx = (*stream).index as usize;
                        streams.push(HlsVariantStream::Audio {
                            index: stream_idx,
                            id: a.id,
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
                "+frag_custom+empty_moov+default_base_moof".to_string(),
            );
        };

        unsafe {
            mux.open(Some(opts))?;
            //av_dump_format(mux.context(), 0, ptr::null_mut(), 0);
        }

        let idx = segments
            .iter()
            .max_by(|a, b| match (a, b) {
                (HlsSegment::Full(a), HlsSegment::Full(b)) => a.index.cmp(&b.index),
                _ => Ordering::Less,
            })
            .and_then(|s| {
                if let HlsSegment::Full(f) = s {
                    Some(f.index + 1)
                } else {
                    None
                }
            })
            .unwrap_or(1);

        let variant = Self {
            name: name.clone(),
            segment_window: 30.0,
            mux,
            streams,
            idx,
            segments,
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

    /// Try to read the playlist and get the segment list
    pub fn try_load_media_seq(dir: &PathBuf) -> Result<Vec<HlsSegment>> {
        let file = dir.join(Self::PLAYLIST_NAME);
        let content = std::fs::read(&file)?;
        let (_, pl) = m3u8_rs::parse_playlist(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse playlist: {}", e))?;
        match pl {
            Playlist::MasterPlaylist(_) => bail!("Invalid MasterPlaylist, expected MediaPlaylist"),
            MediaPlaylist(pl) => {
                let mut idx_ctr = pl.media_sequence;
                let mut partial_ctr = 1;
                let mut ret = Vec::new();

                // map HLS segments from playlist back into internal structs
                for seg in &pl.segments {
                    let mapped_seg = match seg {
                        MediaSegmentType::Full(f) => {
                            let full_seg = HlsSegment::Full(SegmentInfo {
                                index: idx_ctr,
                                duration: f.duration,
                                kind: if f.uri.ends_with(".ts") {
                                    SegmentType::MPEGTS
                                } else {
                                    SegmentType::FMP4
                                },
                                discontinuity: f.discontinuity,
                                sha256: hash_file_sync(&mut File::open(dir.join(&f.uri))?)?,
                                timestamp: f
                                    .program_date_time
                                    .map(|t| t.to_utc())
                                    .unwrap_or_default(),
                            });
                            idx_ctr += 1;
                            partial_ctr = 1; // always reset on full segment
                            full_seg
                        }
                        MediaSegmentType::Partial(p) => {
                            let part_seg = HlsSegment::Partial(PartialSegmentInfo {
                                index: partial_ctr, //assume order is correct
                                parent_index: idx_ctr,
                                // we use byte-range style, so filename always is the same as full segment name
                                parent_kind: if p.uri.ends_with(".ts") {
                                    SegmentType::MPEGTS
                                } else {
                                    SegmentType::FMP4
                                },
                                duration: p.duration,
                                independent: p.independent,
                                byte_range: p.byte_range.as_ref().map(|r| (r.length, r.offset)),
                            });
                            partial_ctr += 1;
                            part_seg
                        }
                        MediaSegmentType::PreloadHint(_) => {
                            // ignore
                            continue;
                        }
                    };
                    ret.push(mapped_seg);
                }
                Ok(ret)
            }
        }
    }

    /// Enable HLS-LL
    pub fn enable_low_latency(&mut self, target_duration: f32) {
        self.low_latency = true;
        self.partial_target_duration = target_duration;
    }

    pub fn segment_length(&self) -> f32 {
        self.segment_length_target.max(2.0)
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
    pub(crate) fn process_packet(
        &mut self,
        pkt: &AvPacketRef,
        var: HlsVariantStream,
    ) -> Result<EgressResult> {
        let mut pkt = pkt.clone();
        let stream_index = var.index() as i32;
        (*pkt).stream_index = stream_index;
        let pkt_stream = unsafe { *(*self.mux.context()).streams.add(stream_index as _) };

        let pkt_q = unsafe { av_q2d(pkt.time_base) };
        let mut result = EgressResult::None;
        let stream_type = unsafe { (*(*pkt_stream).codecpar).codec_type };
        let mut can_split =
            stream_type == AVMEDIA_TYPE_VIDEO && (pkt.flags & AV_PKT_FLAG_KEY == AV_PKT_FLAG_KEY);
        let mut is_ref_pkt = stream_index == self.ref_stream_index;

        if pkt.pts == AV_NOPTS_VALUE {
            can_split = false;
            is_ref_pkt = false;
        }

        if is_ref_pkt {
            let pkt_duration = pkt.duration as f64 * pkt_q;
            trace!(
                "REF PKT index={}, pts={:.3}s, dur={:.3}s, flags={}",
                stream_index,
                pkt.pts as f64 * pkt_q,
                pkt_duration,
                pkt.flags
            );
        }
        if is_ref_pkt && self.packets_written > 0 {
            let pkt_pts = pkt.pts as f64 * pkt_q;
            let cur_duration = pkt_pts - self.current_segment_start;
            let cur_part_duration = pkt_pts - self.current_partial_start;

            let should_end_this_segment = cur_duration >= self.segment_length() as f64;
            let split_seg = can_split && should_end_this_segment;
            let split_partial = stream_type == AVMEDIA_TYPE_VIDEO
                && self.low_latency
                && (cur_part_duration >= self.partial_target_duration as f64 || split_seg);

            if split_partial {
                self.split_partial_segment(pkt_pts, true)?;
                self.next_partial_independent = can_split;
            }
            if split_seg {
                result = self.split_next_seg(pkt_pts, !split_partial)?;
            }
        }

        // write to current segment
        match self.mux.write_packet(&pkt) {
            Ok(r) => r,
            Err(e) => {
                let dst_stream = self
                    .streams
                    .iter()
                    .find(|s| s.index() == stream_index as usize);
                error!(
                    "Error muxing HLS packet: name={}, var={}: {}",
                    self.name,
                    dst_stream
                        .map(|v| v.id().to_string())
                        .unwrap_or("<NO-VAR>".to_string()),
                    e
                );
                return Err(e);
            }
        }
        self.packets_written += 1;

        Ok(result)
    }

    pub fn reset(&mut self) -> Result<()> {
        unsafe { self.mux.close() }
    }

    /// Create a partial segment for LL-HLS
    fn split_partial_segment(&mut self, next_pkt_start: f64, flush: bool) -> Result<()> {
        let ctx = self.mux.context();
        let end_pos = unsafe {
            if flush {
                // First flush the interleave queue (since we use av_interleaved_write_frame for packets)
                av_interleaved_write_frame(ctx, ptr::null_mut());
                // Then flush the muxer's internal buffer to create the fragment (for frag_custom)
                av_write_frame(ctx, ptr::null_mut());
            }
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

        Ok(())
    }

    /// Create initialization segment for fMP4
    fn create_init_segment(&mut self) -> Result<()> {
        if self.segment_type != SegmentType::FMP4 || self.init_segment_path.is_some() {
            return Ok(());
        }

        let init_path = self.out_dir.join("init.mp4").to_string_lossy().to_string();

        // Create a temporary muxer for initialization segment
        let mut init_opts = HashMap::new();
        init_opts.insert(
            "movflags".to_string(),
            "+frag_custom+dash+delay_moov+default_base_moof".to_string(),
        );

        unsafe {
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
        }
        self.init_segment_path = Some("init.mp4".to_string());
        info!("Created fMP4 initialization segment: {}", init_path);

        Ok(())
    }

    /// Reset the muxer state and start the next segment
    fn split_next_seg(&mut self, next_pkt_start: f64, flush: bool) -> Result<EgressResult> {
        let completed_segment_idx = self.idx;
        self.idx += 1;
        self.current_partial_index = 0;

        // Create initialization segment after first segment completion
        // This ensures the init segment has the correct timebase from the encoder
        if self.segment_type == SegmentType::FMP4 && self.init_segment_path.is_none() {
            self.create_init_segment()?;
        }

        unsafe {
            // Manually reset muxer avio
            let ctx = self.mux.context();
            if flush {
                // First flush the interleave queue (since we use av_interleaved_write_frame for packets)
                let ret = av_interleaved_write_frame(ctx, ptr::null_mut());
                bail_ffmpeg!(ret, "Failed to flush interleave queue");
                // Then flush the muxer's internal buffer to create the fragment (for frag_custom)
                let ret = av_write_frame(ctx, ptr::null_mut());
                bail_ffmpeg!(ret, "Failed to write flush frame");
            }
            avio_flush((*ctx).pb);
            avio_closep(&mut (*ctx).pb);
            av_freep(ptr::addr_of_mut!((*ctx).url) as _);

            let next_seg_url = self.map_segment_path(self.idx, self.segment_type);
            (*ctx).url = cstr!(next_seg_url.to_str().unwrap());

            let mut next_io = ptr::null_mut();
            let ret = avio_open(&mut next_io, (*ctx).url, AVIO_FLAG_WRITE);
            bail_ffmpeg!(ret, "Failed to split segment during avio_open!", {
                av_freep(ptr::addr_of_mut!((*ctx).url) as _);
            });
            (*ctx).pb = next_io;
        }

        // Log the completed segment (previous index), not the next one
        let completed_seg_path = self.map_segment_path(completed_segment_idx, self.segment_type);
        let segment_size = completed_seg_path.metadata().map(|m| m.len()).unwrap_or(0);

        let cur_duration = next_pkt_start - self.current_segment_start;
        info!(
            "Finished segment {}/{} [{:.3}s, {:.2} kB, {} pkts, flush={}]",
            self.name,
            completed_seg_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            cur_duration,
            segment_size as f32 / 1024f32,
            self.packets_written,
            flush
        );

        let video_var_id = self
            .video_stream()
            .unwrap_or(self.streams.first().unwrap())
            .id();

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
            discontinuity: false,
            sha256: hash,
            timestamp: Utc::now(),
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
        if let Some(seg_match) = drain_from_hls_segment
            && let Some(drain_pos) = self.segments.iter().position(|e| e == seg_match)
        {
            for seg in self.segments.drain(..drain_pos) {
                if let HlsSegment::Full(seg) = seg {
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
        if self.segment_type == SegmentType::FMP4
            && let Some(ref init_path) = self.init_segment_path
        {
            pl.unknown_tags.push(ExtTag {
                tag: "X-MAP".to_string(),
                rest: Some(format!("URI=\"{}\"", init_path)),
            });
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

        let mut f_out = File::create(self.out_dir.join(Self::PLAYLIST_NAME))?;
        pl.write_to(&mut f_out)?;
        Ok(())
    }

    unsafe fn to_codec_attr(&self) -> Option<String> {
        unsafe {
            let mut codecs = Vec::new();

            // Find video and audio streams and build codec string
            for stream in &self.streams {
                let av_stream = *(*self.mux.context()).streams.add(stream.index());
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
    }

    pub fn to_playlist_variant(&self) -> m3u8_rs::VariantStream {
        unsafe {
            let pes = self.video_stream().unwrap_or(self.streams.first().unwrap());
            let av_stream = *(*self.mux.context()).streams.add(pes.index());
            let codec_par = (*av_stream).codecpar;
            let bitrate = (*codec_par).bit_rate as u64;
            let fps = av_q2d((*codec_par).framerate);
            m3u8_rs::VariantStream {
                is_i_frame: false,
                uri: format!("{}/{}", self.name, Self::PLAYLIST_NAME),
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
