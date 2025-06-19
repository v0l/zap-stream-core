use crate::egress::EgressResult;
use crate::mux::hls::variant::HlsVariant;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::Encoder;
use itertools::Itertools;
use log::trace;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
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

#[derive(Clone, Copy, PartialEq)]
pub enum SegmentType {
    MPEGTS,
    FMP4,
}

pub struct HlsMuxer {
    pub out_dir: PathBuf,
    pub variants: Vec<HlsVariant>,
}

impl HlsMuxer {
    pub fn new<'a>(
        id: &Uuid,
        out_dir: &str,
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
            let var = HlsVariant::new(base.to_str().unwrap(), k, group, segment_type)?;
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
