use crate::egress::{EgressResult, EncoderOrSourceStream};
use crate::mux::hls::variant::HlsVariant;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs::{File, remove_dir_all};
use std::ops::Sub;
use std::path::PathBuf;
use tokio::time::Instant;
use tracing::{trace, warn};
use uuid::Uuid;

mod segment;
mod variant;

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

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum SegmentType {
    MPEGTS,
    FMP4,
}

impl Display for SegmentType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SegmentType::MPEGTS => write!(f, "MPEGTS"),
            SegmentType::FMP4 => write!(f, "fMP4"),
        }
    }
}

pub struct HlsMuxer {
    pub out_dir: PathBuf,
    pub variants: Vec<HlsVariant>,

    last_master_write: Instant,
}

impl HlsMuxer {
    pub const MASTER_PLAYLIST: &'static str = "live.m3u8";

    const MASTER_WRITE_INTERVAL: f32 = 60.0;

    pub fn new<'a>(
        out_dir: PathBuf,
        encoders: impl Iterator<Item = (&'a VariantStream, EncoderOrSourceStream<'a>)>,
        segment_type: SegmentType,
        segment_length: f32,
    ) -> Result<Self> {
        if !out_dir.exists() {
            std::fs::create_dir_all(&out_dir)?;
        }
        let mut vars = Vec::new();
        for (k, group) in &encoders
            .sorted_by(|a, b| a.0.group_id().cmp(&b.0.group_id()))
            .chunk_by(|a| a.0.group_id())
        {
            let mut var = HlsVariant::new(out_dir.clone(), k, group, segment_type, segment_length)?;
            var.enable_low_latency(segment_length / 4.0);
            vars.push(var);
        }

        let mut ret = Self {
            out_dir,
            variants: vars,
            last_master_write: Instant::now(),
        };
        ret.write_master_playlist()?;
        Ok(ret)
    }

    fn write_master_playlist(&mut self) -> Result<()> {
        let mut pl = m3u8_rs::MasterPlaylist::default();
        pl.version = Some(3);
        pl.variants = self
            .variants
            .iter()
            .map(|v| v.to_playlist_variant())
            .collect();

        let mut f_out = File::create(self.out_dir.join(Self::MASTER_PLAYLIST))?;
        pl.write_to(&mut f_out)?;
        self.last_master_write = Instant::now();
        Ok(())
    }

    /// Mux an encoded packet from [Encoder]
    pub unsafe fn mux_packet(
        &mut self,
        pkt: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        unsafe {
            if Instant::now().sub(self.last_master_write).as_secs_f32()
                > Self::MASTER_WRITE_INTERVAL
            {
                self.write_master_playlist()?;
            }
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

    /// Collect all remaining segments that will be deleted during cleanup
    pub fn collect_remaining_segments(&self) -> Vec<crate::egress::EgressSegment> {
        let mut remaining_segments = Vec::new();

        for variant in &self.variants {
            let video_var_id = *variant
                .video_stream()
                .unwrap_or(variant.streams.first().unwrap())
                .id();

            for segment in &variant.segments {
                if let Some(egress_segment) = segment.to_egress_segment(
                    video_var_id,
                    variant.map_segment_path(
                        match segment {
                            segment::HlsSegment::Full(seg) => seg.index,
                            segment::HlsSegment::Partial(seg) => seg.parent_index,
                        },
                        variant.segment_type,
                    ),
                ) {
                    remaining_segments.push(egress_segment);
                }
            }
        }

        remaining_segments
    }
}

impl Drop for HlsMuxer {
    fn drop(&mut self) {
        if let Err(e) = remove_dir_all(&self.out_dir) {
            warn!(
                "Failed to clean up hls dir: {} {}",
                self.out_dir.display(),
                e
            );
        }
    }
}
