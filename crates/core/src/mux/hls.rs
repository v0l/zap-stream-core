use crate::egress::{EgressResult, EgressSegment};
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{bail, ensure, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVMediaType::AVMEDIA_TYPE_VIDEO;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_free, av_opt_set, av_q2d, av_write_frame, avio_close, avio_flush, avio_open, avio_size,
    AVPacket, AVStream, AVIO_FLAG_WRITE, AV_NOPTS_VALUE, AV_PKT_FLAG_KEY,
};
use ffmpeg_rs_raw::{cstr, Encoder, Muxer};
use itertools::Itertools;
use log::{info, trace, warn};
use m3u8_rs::{ByteRange, MediaSegment, MediaSegmentType, Part, PartInf, PreloadHint};
use std::collections::HashMap;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
use std::ptr;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq)]
pub enum SegmentType {
    MPEGTS,
    FMP4,
}

pub enum HlsVariantStream {
    Video {
        group: usize,
        index: usize,
        id: Uuid,
    },
    Audio {
        group: usize,
        index: usize,
        id: Uuid,
    },
    Subtitle {
        group: usize,
        index: usize,
        id: Uuid,
    },
}

impl HlsVariantStream {
    pub fn id(&self) -> &Uuid {
        match self {
            HlsVariantStream::Video { id, .. } => id,
            HlsVariantStream::Audio { id, .. } => id,
            HlsVariantStream::Subtitle { id, .. } => id,
        }
    }

    pub fn index(&self) -> &usize {
        match self {
            HlsVariantStream::Video { index, .. } => index,
            HlsVariantStream::Audio { index, .. } => index,
            HlsVariantStream::Subtitle { index, .. } => index,
        }
    }
}

impl Display for HlsVariantStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HlsVariantStream::Video { index, .. } => write!(f, "v:{}", index),
            HlsVariantStream::Audio { index, .. } => write!(f, "a:{}", index),
            HlsVariantStream::Subtitle { index, .. } => write!(f, "s:{}", index),
        }
    }
}

pub struct HlsVariant {
    /// Name of this variant (720p)
    name: String,
    /// MPEG-TS muxer for this variant
    mux: Muxer,
    /// List of streams ids in this variant
    streams: Vec<HlsVariantStream>,
    /// Segment length in seconds
    segment_length: f32,
    /// Total number of seconds of video to store
    segment_window: f32,
    /// Current segment index
    idx: u64,
    /// Output directory (base)
    out_dir: String,
    /// List of segments to be included in the playlist
    segments: Vec<HlsSegment>,
    /// Type of segments to create
    segment_type: SegmentType,
    /// Ending presentation timestamp
    end_pts: i64,
    /// Current segment duration in seconds (precise accumulation)
    duration: f64,
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
    /// HLS-LL: Current duration in this partial
    current_partial_duration: f64,
    /// HLS-LL: Whether the next partial segment should be marked as independent
    next_partial_independent: bool,
}

#[derive(PartialEq)]
enum HlsSegment {
    Full(SegmentInfo),
    Partial(PartialSegmentInfo),
}

impl HlsSegment {
    fn to_media_segment(&self) -> MediaSegmentType {
        match self {
            HlsSegment::Full(f) => f.to_media_segment(),
            HlsSegment::Partial(p) => p.to_media_segment(),
        }
    }
}

#[derive(PartialEq)]
struct SegmentInfo {
    index: u64,
    duration: f32,
    kind: SegmentType,
}

impl SegmentInfo {
    fn to_media_segment(&self) -> MediaSegmentType {
        MediaSegmentType::Full(MediaSegment {
            uri: self.filename(),
            duration: self.duration,
            ..MediaSegment::default()
        })
    }

    fn filename(&self) -> String {
        HlsVariant::segment_name(self.kind, self.index)
    }
}

#[derive(PartialEq)]
struct PartialSegmentInfo {
    index: u64,
    parent_index: u64,
    parent_kind: SegmentType,
    duration: f64,
    independent: bool,
    byte_range: Option<(u64, Option<u64>)>,
}

impl PartialSegmentInfo {
    fn to_media_segment(&self) -> MediaSegmentType {
        MediaSegmentType::Partial(Part {
            uri: self.filename(),
            duration: self.duration,
            independent: self.independent,
            gap: false,
            byte_range: self.byte_range.map(|r| ByteRange {
                length: r.0,
                offset: r.1,
            }),
        })
    }

    fn filename(&self) -> String {
        HlsVariant::segment_name(self.parent_kind, self.parent_index)
    }

    /// Byte offset where this partial segment ends
    fn end_pos(&self) -> Option<u64> {
        self.byte_range
            .as_ref()
            .map(|(len, start)| start.unwrap_or(0) + len)
    }
}

impl HlsVariant {
    pub fn new<'a>(
        out_dir: &'a str,
        segment_length: f32,
        group: usize,
        encoded_vars: impl Iterator<Item = (&'a VariantStream, &'a Encoder)>,
        segment_type: SegmentType,
    ) -> Result<Self> {
        let name = format!("stream_{}", group);
        let first_seg = Self::map_segment_path(out_dir, &name, 1, segment_type);
        std::fs::create_dir_all(PathBuf::from(&first_seg).parent().unwrap())?;

        let mut opts = HashMap::new();
        if let SegmentType::FMP4 = segment_type {
            opts.insert("fflags".to_string(), "-autobsf".to_string());
            opts.insert(
                "movflags".to_string(),
                "+frag_custom+dash+delay_moov".to_string(),
            );
        };
        let mut mux = unsafe {
            Muxer::builder()
                .with_output_path(
                    first_seg.as_str(),
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
            match var {
                VariantStream::Video(v) => unsafe {
                    let stream = mux.add_stream_encoder(enc)?;
                    let stream_idx = (*stream).index as usize;
                    streams.push(HlsVariantStream::Video {
                        group,
                        index: stream_idx,
                        id: v.id(),
                    });
                    has_video = true;
                    // Always use video stream as reference for segmentation
                    ref_stream_index = stream_idx as _;
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
            }
        }
        ensure!(
            ref_stream_index != -1,
            "No reference stream found, cant create variant"
        );
        trace!(
            "{} will use stream index {} as reference for segmentation",
            name,
            ref_stream_index
        );
        unsafe {
            mux.open(Some(opts))?;
        }
        Ok(Self {
            name: name.clone(),
            segment_length,
            segment_window: 30.0,
            mux,
            streams,
            idx: 1,
            segments: Vec::new(),
            out_dir: out_dir.to_string(),
            segment_type,
            end_pts: AV_NOPTS_VALUE,
            duration: 0.0,
            packets_written: 0,
            ref_stream_index,
            partial_target_duration: 0.33,
            current_partial_index: 0,
            current_partial_duration: 0.0,
            next_partial_independent: false,
            low_latency: false,
        })
    }

    pub fn segment_name(t: SegmentType, idx: u64) -> String {
        match t {
            SegmentType::MPEGTS => format!("{}.ts", idx),
            SegmentType::FMP4 => format!("{}.m4s", idx),
        }
    }

    pub fn out_dir(&self) -> PathBuf {
        PathBuf::from(&self.out_dir).join(&self.name)
    }

    pub fn map_segment_path(out_dir: &str, name: &str, idx: u64, typ: SegmentType) -> String {
        PathBuf::from(out_dir)
            .join(name)
            .join(Self::segment_name(typ, idx))
            .to_string_lossy()
            .to_string()
    }

    /// Process a single packet through the muxer
    unsafe fn process_packet(&mut self, pkt: *mut AVPacket) -> Result<EgressResult> {
        let pkt_stream = *(*self.mux.context())
            .streams
            .add((*pkt).stream_index as usize);

        let mut result = EgressResult::None;
        let stream_type = (*(*pkt_stream).codecpar).codec_type;
        let mut can_split = stream_type == AVMEDIA_TYPE_VIDEO
            && ((*pkt).flags & AV_PKT_FLAG_KEY == AV_PKT_FLAG_KEY);
        let mut is_ref_pkt =
            stream_type == AVMEDIA_TYPE_VIDEO && (*pkt).stream_index == self.ref_stream_index;

        if (*pkt).pts == AV_NOPTS_VALUE {
            can_split = false;
            is_ref_pkt = false;
        }

        // HLS-LL: write prev partial segment
        if self.low_latency && self.current_partial_duration >= self.partial_target_duration as f64
        {
            self.create_partial_segment()?;

            // HLS-LL: Mark next partial as independent if this packet is a keyframe
            if can_split {
                self.next_partial_independent = true;
            }
        }

        // check if current packet is keyframe, flush current segment
        if self.packets_written > 1 && can_split && self.duration >= self.segment_length as f64 {
            result = self.split_next_seg()?;
        }

        // track duration from pts
        if is_ref_pkt {
            if self.end_pts == AV_NOPTS_VALUE {
                self.end_pts = (*pkt).pts;
            }
            let pts_diff = (*pkt).pts - self.end_pts;
            if pts_diff > 0 {
                let time_delta = pts_diff as f64 * av_q2d((*pkt).time_base);
                self.duration += time_delta;
                if self.low_latency {
                    self.current_partial_duration += time_delta;
                }
            }
            self.end_pts = (*pkt).pts;
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
    fn create_partial_segment(&mut self) -> Result<()> {
        let ctx = self.mux.context();
        let end_pos = unsafe {
            avio_flush((*ctx).pb);
            avio_size((*ctx).pb) as u64
        };

        let previous_end_pos = self
            .segments
            .last()
            .and_then(|s| match &s {
                HlsSegment::Partial(p) => p.end_pos(),
                _ => None,
            })
            .unwrap_or(0);
        let independent = self.next_partial_independent;
        let partial_size = end_pos - previous_end_pos;
        let partial_info = PartialSegmentInfo {
            index: self.current_partial_index,
            parent_index: self.idx,
            parent_kind: self.segment_type,
            duration: self.current_partial_duration,
            independent,
            byte_range: Some((partial_size, Some(previous_end_pos))),
        };

        trace!(
            "{} created partial segment {} [{:.3}s, independent={}]",
            self.name,
            partial_info.index,
            partial_info.duration,
            independent
        );
        self.segments.push(HlsSegment::Partial(partial_info));
        self.current_partial_index += 1;
        self.current_partial_duration = 0.0;
        self.next_partial_independent = false;

        self.write_playlist()?;

        Ok(())
    }

    /// Reset the muxer state and start the next segment
    unsafe fn split_next_seg(&mut self) -> Result<EgressResult> {
        let completed_segment_idx = self.idx;
        self.idx += 1;

        // Manually reset muxer avio
        let ctx = self.mux.context();
        av_write_frame(ctx, ptr::null_mut());
        avio_flush((*ctx).pb);
        avio_close((*ctx).pb);
        av_free((*ctx).url as *mut _);

        let next_seg_url =
            Self::map_segment_path(&self.out_dir, &self.name, self.idx, self.segment_type);
        (*ctx).url = cstr!(next_seg_url.as_str());

        let ret = avio_open(&mut (*ctx).pb, (*ctx).url, AVIO_FLAG_WRITE);
        if ret < 0 {
            bail!("Failed to re-init avio");
        }

        // tell muxer it needs to write headers again
        av_opt_set(
            (*ctx).priv_data,
            cstr!("events_flags"),
            cstr!("resend_headers"),
            0,
        );

        // Log the completed segment (previous index), not the next one
        let completed_seg_path = Self::map_segment_path(
            &self.out_dir,
            &self.name,
            completed_segment_idx,
            self.segment_type,
        );
        let completed_segment_path = PathBuf::from(&completed_seg_path);
        let segment_size = completed_segment_path
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0);
        info!(
            "Finished segment {} [{:.3}s, {:.2} kB, {} pkts]",
            completed_segment_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            self.duration,
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
                path: PathBuf::from(Self::map_segment_path(
                    &self.out_dir,
                    &self.name,
                    seg.index,
                    self.segment_type,
                )),
            })
            .collect();

        // emit result of the previously completed segment,
        let created = EgressSegment {
            variant: video_var_id,
            idx: completed_segment_idx,
            duration: self.duration as f32,
            path: completed_segment_path,
        };

        if let Err(e) = self.push_segment(completed_segment_idx, self.duration as f32) {
            warn!("Failed to update playlist: {}", e);
        }

        // Reset counters for next segment
        self.packets_written = 0;
        self.duration = 0.0;

        Ok(EgressResult::Segments {
            created: vec![created],
            deleted,
        })
    }

    fn video_stream(&self) -> Option<&HlsVariantStream> {
        self.streams
            .iter()
            .find(|a| matches!(*a, HlsVariantStream::Video { .. }))
    }

    /// Add a new segment to the variant and return a list of deleted segments
    fn push_segment(&mut self, idx: u64, duration: f32) -> Result<()> {
        self.segments.push(HlsSegment::Full(SegmentInfo {
            index: idx,
            duration,
            kind: self.segment_type,
        }));

        self.write_playlist()
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
                let seg_dir = self.out_dir();
                for seg in self.segments.drain(..drain_pos) {
                    match seg {
                        HlsSegment::Full(seg) => {
                            let seg_path = seg_dir.join(seg.filename());
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

    fn write_playlist(&mut self) -> Result<()> {
        if self.segments.is_empty() {
            return Ok(()); // Don't write empty playlists
        }

        let mut pl = m3u8_rs::MediaPlaylist::default();
        pl.target_duration = (self.segment_length.ceil() as u64).max(1);
        pl.segments = self.segments.iter().map(|s| s.to_media_segment()).collect();

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
        pl.version = Some(if self.low_latency { 6 } else { 3 });
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

        let mut f_out = File::create(self.out_dir().join("live.m3u8"))?;
        pl.write_to(&mut f_out)?;
        Ok(())
    }

    /// https://git.ffmpeg.org/gitweb/ffmpeg.git/blob/HEAD:/libavformat/hlsenc.c#l351
    unsafe fn to_codec_attr(&self, stream: *mut AVStream) -> Option<String> {
        let p = (*stream).codecpar;
        if (*p).codec_id == AV_CODEC_ID_H264 {
            let data = (*p).extradata;
            if !data.is_null() {
                let mut id_ptr = ptr::null_mut();
                let ds: *mut u16 = data as *mut u16;
                if (*ds) == 1 && (*data.add(4)) & 0x1F == 7 {
                    id_ptr = data.add(5);
                } else if (*ds) == 1 && (*data.add(3)) & 0x1F == 7 {
                    id_ptr = data.add(4);
                } else if *data.add(0) == 1 {
                    id_ptr = data.add(1);
                } else {
                    return None;
                }

                return Some(format!(
                    "avc1.{}",
                    hex::encode([*id_ptr.add(0), *id_ptr.add(1), *id_ptr.add(2)])
                ));
            }
        }
        None
    }

    pub fn to_playlist_variant(&self) -> m3u8_rs::VariantStream {
        unsafe {
            let pes = self.video_stream().unwrap_or(self.streams.first().unwrap());
            let av_stream = *(*self.mux.context()).streams.add(*pes.index());
            let codec_par = (*av_stream).codecpar;
            m3u8_rs::VariantStream {
                is_i_frame: false,
                uri: format!("{}/live.m3u8", self.name),
                bandwidth: 0,
                average_bandwidth: Some((*codec_par).bit_rate as u64),
                codecs: self.to_codec_attr(av_stream),
                resolution: Some(m3u8_rs::Resolution {
                    width: (*codec_par).width as _,
                    height: (*codec_par).height as _,
                }),
                frame_rate: Some(av_q2d((*codec_par).framerate)),
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

pub struct HlsMuxer {
    pub out_dir: PathBuf,
    pub variants: Vec<HlsVariant>,
}

impl HlsMuxer {
    pub fn new<'a>(
        id: &Uuid,
        out_dir: &str,
        segment_length: f32,
        encoders: impl Iterator<Item = (&'a VariantStream, &'a Encoder)>,
        segment_type: SegmentType,
    ) -> Result<Self> {
        let base = PathBuf::from(out_dir).join(id.to_string());

        if !base.exists() {
            std::fs::create_dir_all(&base)?;
        }
        let mut vars = Vec::new();
        for (k, group) in &encoders
            .sorted_by(|a, b| a.0.group_id().cmp(&b.0.group_id()))
            .chunk_by(|a| a.0.group_id())
        {
            let var = HlsVariant::new(
                base.to_str().unwrap(),
                segment_length,
                k,
                group,
                segment_type,
            )?;
            vars.push(var);
        }

        let ret = Self {
            out_dir: base,
            variants: vars,
        };
        ret.write_master_playlist()?;
        Ok(ret)
    }

    fn write_master_playlist(&self) -> Result<()> {
        let mut pl = m3u8_rs::MasterPlaylist::default();
        pl.version = Some(3);
        pl.variants = self
            .variants
            .iter()
            .map(|v| v.to_playlist_variant())
            .collect();

        let mut f_out = File::create(self.out_dir.join("live.m3u8"))?;
        pl.write_to(&mut f_out)?;
        Ok(())
    }

    /// Mux an encoded packet from [Encoder]
    pub unsafe fn mux_packet(
        &mut self,
        pkt: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        for var in self.variants.iter_mut() {
            if let Some(vs) = var.streams.iter().find(|s| s.id() == variant) {
                // very important for muxer to know which stream this pkt belongs to
                (*pkt).stream_index = *vs.index() as _;
                return var.process_packet(pkt);
            }
        }

        // This HLS muxer doesn't handle this variant, return None instead of failing
        // This can happen when multiple egress handlers are configured with different variant sets
        trace!(
            "HLS muxer received packet for variant {} which it doesn't handle",
            variant
        );
        Ok(EgressResult::None)
    }
}
