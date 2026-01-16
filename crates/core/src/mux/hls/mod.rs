use crate::egress::{EgressResult, EncoderVariantGroup};
use crate::mux::SegmentType;
use crate::mux::hls::variant::HlsVariant;
use anyhow::Result;
use ffmpeg_rs_raw::AvPacketRef;
use itertools::Itertools;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;
use tracing::log::warn;
use tracing::trace;
use uuid::Uuid;

mod segment;
mod variant;

#[derive(Clone)]
pub enum HlsVariantStream {
    Video { index: usize, id: Uuid },
    Audio { index: usize, id: Uuid },
    Subtitle { index: usize, id: Uuid },
}

impl HlsVariantStream {
    pub fn id(&self) -> Uuid {
        match self {
            HlsVariantStream::Video { id, .. } => *id,
            HlsVariantStream::Audio { id, .. } => *id,
            HlsVariantStream::Subtitle { id, .. } => *id,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            HlsVariantStream::Video { index, .. } => *index,
            HlsVariantStream::Audio { index, .. } => *index,
            HlsVariantStream::Subtitle { index, .. } => *index,
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

pub struct HlsMuxer {
    pub out_dir: PathBuf,
    pub variants: Vec<HlsVariant>,

    last_master_write: Instant,
}

impl HlsMuxer {
    pub const MASTER_PLAYLIST: &'static str = "live.m3u8";

    pub fn new(
        out_dir: PathBuf,
        encoders: &Vec<EncoderVariantGroup>,
        segment_type: SegmentType,
        segment_length: f32,
    ) -> Result<Self> {
        if !out_dir.exists() {
            std::fs::create_dir_all(&out_dir)?;
        }
        let mut vars = Vec::new();
        for g in encoders {
            let var = HlsVariant::new(out_dir.clone(), g, segment_type, segment_length)?;
            //var.enable_low_latency(segment_length / 4.0);
            vars.push(var);
        }

        // force all variants to have the same segment length
        if let Some(max_seg_duration) = vars
            .iter()
            .map(|s| s.segment_length_target)
            .sorted_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .last()
            && max_seg_duration != segment_length
        {
            warn!(
                "Forcing segment length to {:.2}s from {:.2}s",
                max_seg_duration, segment_length
            );
            vars.iter_mut()
                .for_each(|s| s.segment_length_target = max_seg_duration);
        }

        let mut ret = Self {
            out_dir,
            variants: vars,
            last_master_write: Instant::now(),
        };
        ret.write_master_playlist()?;
        Ok(ret)
    }

    pub fn open(&mut self) -> Result<()> {
        for var in &mut self.variants {
            var.open()?;
        }
        Ok(())
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
    pub fn mux_packet(&mut self, pkt: AvPacketRef, variant: &Uuid) -> Result<EgressResult> {
        // Process packet for ALL variants that contain this stream
        // (same audio stream can be shared across multiple HLS variant groups)
        let mut created = Vec::new();
        let mut deleted = Vec::new();
        let mut found = false;

        for var in self.variants.iter_mut() {
            if let Some(vs) = var.streams.iter().find(|s| s.id() == *variant) {
                found = true;
                if let EgressResult::Segments {
                    created: c,
                    deleted: d,
                } = var.process_packet(&pkt, vs.clone())?
                {
                    created.extend(c);
                    deleted.extend(d);
                }
            }
        }

        if !found {
            // This HLS muxer doesn't handle this variant, return None instead of failing
            // This can happen when multiple egress handlers are configured with different variant sets
            trace!(
                "HLS muxer received packet for variant {} which it doesn't handle",
                variant
            );
            return Ok(EgressResult::None);
        }

        if created.is_empty() && deleted.is_empty() {
            Ok(EgressResult::None)
        } else {
            Ok(EgressResult::Segments { created, deleted })
        }
    }

    /// Collect all remaining segments that will be deleted during cleanup
    pub fn collect_remaining_segments(&self) -> Vec<crate::egress::EgressSegment> {
        let mut remaining_segments = Vec::new();

        for variant in &self.variants {
            let video_var_id = variant
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
