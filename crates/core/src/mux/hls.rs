use crate::egress::{EgressResult, EgressSegment};
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{bail, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVMediaType::AVMEDIA_TYPE_VIDEO;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_free, av_opt_set, av_q2d, av_write_frame, avio_flush, avio_open, AVPacket, AVStream,
    AVIO_FLAG_WRITE, AV_PKT_FLAG_KEY,
};
use ffmpeg_rs_raw::{cstr, Encoder, Muxer};
use itertools::Itertools;
use log::{info, warn};
use m3u8_rs::MediaSegment;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
use std::ptr;
use uuid::Uuid;

#[derive(Clone, Copy)]
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
    pub name: String,
    /// MPEG-TS muxer for this variant
    pub mux: Muxer,
    /// List of streams ids in this variant
    pub streams: Vec<HlsVariantStream>,
    /// Segment length in seconds
    pub segment_length: f32,
    /// Total number of segments to store for this variant
    pub segment_window: Option<u16>,
    /// Current segment index
    pub idx: u64,
    /// Current segment start time in seconds (duration)
    pub pkt_start: f32,
    /// Output directory (base)
    pub out_dir: String,
    /// List of segments to be included in the playlist
    pub segments: Vec<SegmentInfo>,
    /// Type of segments to create
    pub segment_type: SegmentType,
}

struct SegmentInfo {
    pub index: u64,
    pub duration: f32,
    pub kind: SegmentType,
}

impl SegmentInfo {
    fn to_media_segment(&self) -> MediaSegment {
        MediaSegment {
            uri: self.filename(),
            duration: self.duration,
            title: None,
            ..MediaSegment::default()
        }
    }

    fn filename(&self) -> String {
        HlsVariant::segment_name(self.kind, self.index)
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
        for (var, enc) in encoded_vars {
            match var {
                VariantStream::Video(v) => unsafe {
                    let stream = mux.add_stream_encoder(enc)?;
                    streams.push(HlsVariantStream::Video {
                        group,
                        index: (*stream).index as usize,
                        id: v.id(),
                    })
                },
                VariantStream::Audio(a) => unsafe {
                    let stream = mux.add_stream_encoder(enc)?;
                    streams.push(HlsVariantStream::Audio {
                        group,
                        index: (*stream).index as usize,
                        id: a.id(),
                    })
                },
                VariantStream::Subtitle(s) => unsafe {
                    let stream = mux.add_stream_encoder(enc)?;
                    streams.push(HlsVariantStream::Subtitle {
                        group,
                        index: (*stream).index as usize,
                        id: s.id(),
                    })
                },
                _ => panic!("unsupported variant stream"),
            }
        }
        unsafe {
            mux.open(Some(opts))?;
        }
        Ok(Self {
            name: name.clone(),
            segment_length,
            segment_window: Some(10), //TODO: configure window
            mux,
            streams,
            idx: 1,
            pkt_start: 0.0,
            segments: Vec::from([SegmentInfo {
                index: 1,
                duration: segment_length,
                kind: segment_type,
            }]),
            out_dir: out_dir.to_string(),
            segment_type,
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

    /// Mux a packet created by the encoder for this variant
    pub unsafe fn mux_packet(&mut self, pkt: *mut AVPacket) -> Result<EgressResult> {
        let pkt_q = av_q2d((*pkt).time_base);
        // time of this packet in seconds
        let pkt_time = (*pkt).pts as f32 * pkt_q as f32;
        // what segment this pkt should be in (index)
        let pkt_seg = 1 + (pkt_time / self.segment_length).floor() as u64;

        let mut result = EgressResult::None;
        let pkt_stream = *(*self.mux.context())
            .streams
            .add((*pkt).stream_index as usize);
        let can_split = (*pkt).flags & AV_PKT_FLAG_KEY == AV_PKT_FLAG_KEY
            && (*(*pkt_stream).codecpar).codec_type == AVMEDIA_TYPE_VIDEO;
        if pkt_seg != self.idx && can_split {
            result = self.split_next_seg(pkt_time)?;
        }
        self.mux.write_packet(pkt)?;
        Ok(result)
    }

    pub unsafe fn reset(&mut self) -> Result<()> {
        self.mux.close()
    }

    /// Reset the muxer state and start the next segment
    unsafe fn split_next_seg(&mut self, pkt_time: f32) -> Result<EgressResult> {
        self.idx += 1;

        // Manually reset muxer avio
        let ctx = self.mux.context();
        av_write_frame(ctx, ptr::null_mut());
        avio_flush((*ctx).pb);
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

        let duration = pkt_time - self.pkt_start;
        info!("Writing segment {} [{}s]", &next_seg_url, duration);
        if let Err(e) = self.push_segment(self.idx, duration) {
            warn!("Failed to update playlist: {}", e);
        }

        /// Get the video variant for this group
        /// since this could actually be audio which would not be useful for
        /// [Overseer] impl
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
        let prev_seg = self.idx - 1;
        let created = EgressSegment {
            variant: video_var_id,
            idx: prev_seg,
            duration,
            path: PathBuf::from(Self::map_segment_path(
                &self.out_dir,
                &self.name,
                prev_seg,
                self.segment_type,
            )),
        };
        self.pkt_start = pkt_time;
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
        self.segments.push(SegmentInfo {
            index: idx,
            duration,
            kind: self.segment_type,
        });

        self.write_playlist()
    }

    /// Delete segments which are too old
    fn clean_segments(&mut self) -> Result<Vec<SegmentInfo>> {
        const MAX_SEGMENTS: usize = 10;

        let mut ret = vec![];
        if self.segments.len() > MAX_SEGMENTS {
            let n_drain = self.segments.len() - MAX_SEGMENTS;
            let seg_dir = self.out_dir();
            for seg in self.segments.drain(..n_drain) {
                // delete file
                let seg_path = seg_dir.join(seg.filename());
                if let Err(e) = std::fs::remove_file(&seg_path) {
                    warn!(
                        "Failed to remove segment file: {} {}",
                        seg_path.display(),
                        e
                    );
                }
                ret.push(seg);
            }
        }
        Ok(ret)
    }

    fn write_playlist(&mut self) -> Result<()> {
        let mut pl = m3u8_rs::MediaPlaylist::default();
        pl.target_duration = self.segment_length as u64;
        pl.segments = self.segments.iter().map(|s| s.to_media_segment()).collect();
        pl.version = Some(3);
        pl.media_sequence = self.segments.first().map(|s| s.index).unwrap_or(0);

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
                return var.mux_packet(pkt);
            }
        }
        bail!("Packet doesnt match any variants");
    }
}
