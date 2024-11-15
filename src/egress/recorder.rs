use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::{Encoder, Muxer};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressConfig, EgressResult};

pub struct RecorderEgress {
    id: Uuid,
    config: EgressConfig,
    muxer: Muxer,
}

impl RecorderEgress {
    pub fn new<'a>(
        config: EgressConfig,
        variants: impl Iterator<Item = &'a Encoder>,
    ) -> Result<Self> {
        let id = Uuid::new_v4();
        let base = PathBuf::from(&config.out_dir).join(id.to_string());

        let out_file = base.join("recording.ts");
        fs::create_dir_all(&base)?;

        let muxer = unsafe {
            let mut m = Muxer::builder()
                .with_output_path(out_file.to_str().unwrap(), None)?
                .build()?;
            for var in variants {
                m.add_stream_encoder(var)?;
            }
            m.open(None)?;
            m
        };
        Ok(Self { id, config, muxer })
    }
}

impl Egress for RecorderEgress {
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        if self.config.variants.contains(variant) {
            self.muxer.write_packet(packet)?;
        }
        Ok(EgressResult::None)
    }
}
