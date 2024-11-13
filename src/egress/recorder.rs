use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::{Encoder, Muxer};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressConfig};

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

        let out_file = base.join("recording.mp4");
        fs::create_dir_all(&base)?;

        let mut opt = HashMap::new();
        opt.insert(
            "movflags".to_string(),
            "+dash+delay_moov+skip_sidx+skip_trailer".to_string(),
        );

        let muxer = unsafe {
            let mut m = Muxer::new().with_output(&out_file, None, Some(opt))?;
            for var in variants {
                m.add_stream_encoder(var)?;
            }
            m.open()?;
            m
        };
        Ok(Self { id, config, muxer })
    }
}

impl Egress for RecorderEgress {
    unsafe fn process_pkt(&mut self, packet: *mut AVPacket, variant: &Uuid) -> Result<()> {
        if self.config.variants.contains(variant) {
            self.muxer.write_packet(packet)
        } else {
            Ok(())
        }
    }
}
