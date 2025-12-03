use anyhow::Result;
use ffmpeg_rs_raw::AvPacketRef;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult, EncoderVariantGroup};
use crate::mux::{HlsMuxer, SegmentType};

/// Alias the muxer directly
pub struct HlsEgress {
    mux: HlsMuxer,
}

impl HlsEgress {
    pub const PATH: &'static str = "hls";

    pub fn new(
        out_dir: PathBuf,
        encoders: &Vec<EncoderVariantGroup>,
        segment_type: SegmentType,
        segment_length: f32,
    ) -> Result<Self> {
        Ok(Self {
            mux: HlsMuxer::new(
                out_dir.join(Self::PATH),
                encoders,
                segment_type,
                segment_length,
            )?,
        })
    }
}

impl Egress for HlsEgress {
    fn process_pkt(&mut self, packet: AvPacketRef, variant: &Uuid) -> Result<EgressResult> {
        self.mux.mux_packet(packet, variant)
    }

    fn reset(&mut self) -> Result<EgressResult> {
        let remaining_segments = self.mux.collect_remaining_segments();

        for var in &mut self.mux.variants {
            var.reset()?
        }

        Ok(EgressResult::Segments {
            created: vec![],
            deleted: remaining_segments,
        })
    }
}
