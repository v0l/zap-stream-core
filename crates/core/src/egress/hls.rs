use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::Encoder;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult};
use crate::mux::{HlsMuxer, SegmentType};
use crate::variant::VariantStream;

/// Alias the muxer directly
pub struct HlsEgress {
    mux: HlsMuxer,
}

impl HlsEgress {
    pub const PATH: &'static str = "hls";

    pub fn new<'a>(
        out_dir: PathBuf,
        encoders: impl Iterator<Item = (&'a VariantStream, &'a Encoder)>,
        segment_type: SegmentType,
    ) -> Result<Self> {
        Ok(Self {
            mux: HlsMuxer::new(out_dir.join(Self::PATH), encoders, segment_type)?,
        })
    }
}

impl Egress for HlsEgress {
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        self.mux.mux_packet(packet, variant)
    }

    unsafe fn reset(&mut self) -> Result<()> {
        for var in &mut self.mux.variants {
            var.reset()?
        }
        Ok(())
    }
}
