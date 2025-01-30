use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult};
use crate::mux::HlsMuxer;

/// Alias the muxer directly
pub type HlsEgress = HlsMuxer;

impl Egress for HlsMuxer {
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        self.mux_packet(packet, variant)
    }

    unsafe fn reset(&mut self) -> Result<()> {
        for var in &mut self.variants {
            var.reset()?
        }
        Ok(())
    }
}
