use crate::egress::{EgressResult, EncoderVariant, EncoderVariantGroup};
use crate::mux::SegmentType;
use crate::mux::hls::variant::HlsVariant;
use crate::variant::VariantStream;
use anyhow::Result;
use ffmpeg_rs_raw::AvPacketRef;
use itertools::Itertools;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;
use tracing::{trace, warn};
use uuid::Uuid;

/// Returns true if this encoder variant carries an audio stream
fn is_audio_variant(v: &EncoderVariant) -> bool {
    matches!(
        v.variant,
        VariantStream::Audio(_) | VariantStream::CopyAudio(_)
    )
}

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
        match segment_type {
            SegmentType::FMP4 => {
                // CMAF: extract the (deduplicated) audio streams into their own
                // rendition group so the shared audio track is only stored/served once
                // instead of being muxed into every video variant. The audio variant's
                // UUID is unique already, so we use it directly as the rendition group
                // id (and as the on-disk directory name).
                let mut audio_seen: Vec<Uuid> = Vec::new();
                for g in encoders {
                    for s in g.streams.iter().filter(|s| is_audio_variant(s)) {
                        let id = s.variant.id();
                        if audio_seen.contains(&id) {
                            continue;
                        }
                        audio_seen.push(id);
                        let var = HlsVariant::new(
                            out_dir.clone(),
                            id.to_string(),
                            &[s],
                            segment_type,
                            segment_length,
                            Some(id.to_string()),
                        )?;
                        vars.push(var);
                    }
                }

                // Create the main (video) variants with audio stripped, each
                // referencing the shared audio rendition group by its UUID.
                for g in encoders {
                    let media: Vec<&EncoderVariant> =
                        g.streams.iter().filter(|s| !is_audio_variant(s)).collect();
                    if media.is_empty() {
                        // pure audio-only group, already handled above
                        continue;
                    }
                    let audio_group = g
                        .streams
                        .iter()
                        .find(|s| is_audio_variant(s))
                        .map(|s| s.variant.id().to_string());
                    let var = HlsVariant::new(
                        out_dir.clone(),
                        g.id.to_string(),
                        &media,
                        segment_type,
                        segment_length,
                        audio_group,
                    )?;
                    vars.push(var);
                }
            }
            SegmentType::MPEGTS => {
                // MPEG-TS keeps audio muxed together with video in each variant
                for g in encoders {
                    let streams: Vec<&EncoderVariant> = g.streams.iter().collect();
                    let var = HlsVariant::new(
                        out_dir.clone(),
                        g.id.to_string(),
                        &streams,
                        segment_type,
                        segment_length,
                        None,
                    )?;
                    vars.push(var);
                }
            }
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

        // Low-latency HLS requires byte-range addressable partial segments; we only
        // enable it for fMP4 output where partial fragments are well supported by players.
        // This must happen AFTER segment lengths are forced equal so every variant
        // derives its PART-TARGET from the same (final) segment length.
        if segment_type == SegmentType::FMP4 {
            for var in vars.iter_mut() {
                let part_target = var.partial_segment_length();
                var.enable_low_latency(part_target);
            }
        }

        Ok(Self {
            out_dir,
            variants: vars,
            last_master_write: Instant::now(),
        })
    }

    pub fn open(&mut self) -> Result<()> {
        for var in &mut self.variants {
            var.open()?;
        }
        self.write_master_playlist()?;
        Ok(())
    }

    fn write_master_playlist(&mut self) -> Result<()> {
        let mut pl = m3u8_rs::MasterPlaylist::default();
        // fMP4/CMAF (EXT-X-MAP + audio rendition groups) and LL-HLS require HLS protocol v6+
        let is_fmp4 = self
            .variants
            .iter()
            .any(|v| v.segment_type == SegmentType::FMP4);
        pl.version = Some(if is_fmp4 { 6 } else { 3 });
        // EXT-X-MEDIA entries for shared audio renditions
        pl.alternatives = self
            .variants
            .iter()
            .filter_map(|v| v.to_alternative_media())
            .collect();
        pl.variants = self
            .variants
            .iter()
            .filter_map(|v| v.to_playlist_variant())
            .collect();

        let pl_path = self.out_dir.join(Self::MASTER_PLAYLIST);
        let tmp_path = self.out_dir.join(format!("{}.tmp", Self::MASTER_PLAYLIST));
        {
            let mut f_out = File::create(&tmp_path)?;
            pl.write_to(&mut f_out)?;
        }
        // Atomic rename so concurrent players/CDN never observe a truncated playlist
        std::fs::rename(&tmp_path, &pl_path)?;
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
                    variant.is_audio_only(),
                ) {
                    remaining_segments.push(egress_segment);
                }
            }
        }

        remaining_segments
    }
}
