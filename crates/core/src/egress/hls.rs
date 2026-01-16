use anyhow::Result;
use ffmpeg_rs_raw::AvPacketRef;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult};
use crate::mux::HlsMuxer;

pub const HLS_EGRESS_PATH: &'static str = "hls";

impl Egress for HlsMuxer {
    fn process_pkt(&mut self, packet: AvPacketRef, variant: &Uuid) -> Result<EgressResult> {
        self.mux_packet(packet, variant)
    }

    fn reset(&mut self) -> Result<EgressResult> {
        let remaining_segments = self.collect_remaining_segments();

        for var in &mut self.variants {
            var.reset()?
        }

        Ok(EgressResult::Segments {
            created: vec![],
            deleted: remaining_segments,
        })
    }
}
