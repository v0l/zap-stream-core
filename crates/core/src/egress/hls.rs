use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult, EncoderOrSourceStream};
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
        encoders: impl Iterator<Item = (&'a VariantStream, EncoderOrSourceStream<'a>)>,
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
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        unsafe { self.mux.mux_packet(packet, variant) }
    }

    unsafe fn reset(&mut self) -> Result<EgressResult> {
        unsafe {
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
}
