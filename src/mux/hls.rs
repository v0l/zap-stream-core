use crate::egress::NewSegment;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{bail, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_free, av_opt_set, av_q2d, av_write_frame, avio_flush, avio_open, AVPacket, AVIO_FLAG_WRITE,
    AV_PKT_FLAG_KEY,
};
use ffmpeg_rs_raw::{cstr, Encoder, Muxer};
use itertools::Itertools;
use log::{info, warn};
use m3u8_rs::MediaSegment;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
use std::ptr;
use uuid::Uuid;

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
    /// Current segment index
    pub idx: u64,
    /// Output directory (base)
    pub out_dir: String,
    /// List of segments to be included in the playlist
    pub segments: Vec<SegmentInfo>,
}

struct SegmentInfo(u64, f32);

impl SegmentInfo {
    fn to_media_segment(&self) -> MediaSegment {
        MediaSegment {
            uri: HlsVariant::segment_name(self.0),
            duration: self.1,
            title: Some("no desc".to_string()),
            ..MediaSegment::default()
        }
    }

    fn filename(&self) -> String {
        HlsVariant::segment_name(self.0)
    }
}

impl HlsVariant {
    pub fn new<'a>(
        out_dir: &'a str,
        segment_length: f32,
        group: usize,
        encoded_vars: impl Iterator<Item = (&'a VariantStream, &'a Encoder)>,
    ) -> Result<Self> {
        let name = format!("stream_{}", group);
        let first_seg = Self::map_segment_path(out_dir, &name, 1);
        std::fs::create_dir_all(PathBuf::from(&first_seg).parent().unwrap())?;

        let mut mux = unsafe {
            Muxer::builder()
                .with_output_path(first_seg.as_str(), Some("mpegts"))?
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
            mux.open(None)?;
        }
        Ok(Self {
            name: name.clone(),
            segment_length,
            mux,
            streams,
            idx: 1,
            segments: Vec::from([SegmentInfo(1, segment_length)]),
            out_dir: out_dir.to_string(),
        })
    }

    pub fn segment_name(idx: u64) -> String {
        format!("{}.ts", idx)
    }

    pub fn out_dir(&self) -> PathBuf {
        PathBuf::from(&self.out_dir).join(&self.name)
    }

    pub fn map_segment_path(out_dir: &str, name: &str, idx: u64) -> String {
        PathBuf::from(out_dir)
            .join(name)
            .join(Self::segment_name(idx))
            .to_string_lossy()
            .to_string()
    }

    /// Mux a packet created by the encoder for this variant
    pub unsafe fn mux_packet(&mut self, pkt: *mut AVPacket) -> Result<Option<NewSegment>> {
        // time of this packet in seconds
        let pkt_time = (*pkt).pts as f32 * av_q2d((*pkt).time_base) as f32;
        // what segment this pkt should be in (index)
        let pkt_seg = 1 + (pkt_time / self.segment_length).floor() as u64;

        let mut result = None;
        let can_split = (*pkt).flags & AV_PKT_FLAG_KEY == AV_PKT_FLAG_KEY;
        if pkt_seg != self.idx && can_split {
            result = Some(self.split_next_seg()?);
        }
        self.mux.write_packet(pkt)?;
        Ok(result)
    }

    unsafe fn split_next_seg(&mut self) -> Result<NewSegment> {
        self.idx += 1;

        // Manually reset muxer avio
        let ctx = self.mux.context();
        av_write_frame(ctx, ptr::null_mut());
        avio_flush((*ctx).pb);
        av_free((*ctx).url as *mut _);

        let next_seg_url = Self::map_segment_path(&*self.out_dir, &self.name, self.idx);
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

        // TODO: calc actual duration
        let duration = 2.0;
        info!("Writing segment {}", &next_seg_url);
        if let Err(e) = self.add_segment(self.idx, duration) {
            warn!("Failed to update playlist: {}", e);
        }

        /// Get the video variant for this group
        /// since this could actually be audio which would not be useful for
        /// [Overseer] impl
        let video_var = self
            .streams
            .iter()
            .find(|a| matches!(*a, HlsVariantStream::Video { .. }))
            .map_or(Default::default(), |v| v.id().clone());

        // emit result of the previously completed segment,
        let prev_seg = self.idx - 1;
        Ok(NewSegment {
            variant: video_var,
            idx: prev_seg,
            duration,
            path: PathBuf::from(Self::map_segment_path(&*self.out_dir, &self.name, prev_seg)),
        })
    }

    fn add_segment(&mut self, idx: u64, duration: f32) -> Result<()> {
        self.segments.push(SegmentInfo(idx, duration));

        const MAX_SEGMENTS: usize = 10;

        if self.segments.len() > MAX_SEGMENTS {
            let n_drain = self.segments.len() - MAX_SEGMENTS;
            let seg_dir = PathBuf::from(self.out_dir());
            for seg in self.segments.drain(..n_drain) {
                // delete file
                let seg_path = seg_dir.join(seg.filename());
                std::fs::remove_file(seg_path)?;
            }
        }
        self.write_playlist()
    }

    fn write_playlist(&mut self) -> Result<()> {
        let mut pl = m3u8_rs::MediaPlaylist::default();
        pl.target_duration = self.segment_length as u64;
        pl.segments = self.segments.iter().map(|s| s.to_media_segment()).collect();
        pl.version = Some(3);
        pl.media_sequence = self.segments.first().map(|s| s.0).unwrap_or(0);

        let mut f_out = File::create(self.out_dir().join("live.m3u8"))?;
        pl.write_to(&mut f_out)?;
        Ok(())
    }
}

pub struct HlsMuxer {
    variants: Vec<HlsVariant>,
}

impl HlsMuxer {
    pub fn new<'a>(
        out_dir: &str,
        segment_length: f32,
        encoders: impl Iterator<Item = (&'a VariantStream, &'a Encoder)>,
    ) -> Result<Self> {
        let id = Uuid::new_v4();
        let base = PathBuf::from(out_dir)
            .join(id.to_string())
            .to_string_lossy()
            .to_string();

        let mut vars = Vec::new();
        for (k, group) in &encoders
            .sorted_by(|a, b| a.0.group_id().cmp(&b.0.group_id()))
            .chunk_by(|a| a.0.group_id())
        {
            let var = HlsVariant::new(&base, segment_length, k, group)?;
            vars.push(var);
        }

        Ok(Self { variants: vars })
    }

    /// Mux an encoded packet from [Encoder]
    pub unsafe fn mux_packet(
        &mut self,
        pkt: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<Option<NewSegment>> {
        for var in self.variants.iter_mut() {
            if var.streams.iter().any(|s| s.id() == variant) {
                return var.mux_packet(pkt);
            }
        }
        bail!("Packet doesnt match any variants");
    }
}
