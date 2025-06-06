use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::{Encoder, Muxer};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult};
use crate::variant::{StreamMapping, VariantStream};

pub struct RecorderEgress {
    /// Pipeline ID
    id: Uuid,
    /// Internal muxer writing the output packets
    muxer: Muxer,
    /// Mapping from Variant ID to stream index
    var_map: HashMap<Uuid, i32>,
}

impl RecorderEgress {
    pub fn new<'a>(
        id: &Uuid,
        out_dir: &str,
        variants: impl Iterator<Item = (&'a VariantStream, &'a Encoder)>,
    ) -> Result<Self> {
        let base = PathBuf::from(out_dir).join(id.to_string());

        let out_file = base.join("recording.ts");
        fs::create_dir_all(&base)?;

        let mut var_map = HashMap::new();
        let muxer = unsafe {
            let mut m = Muxer::builder()
                .with_output_path(out_file.to_str().unwrap(), None)?
                .build()?;
            for (var, enc) in variants {
                let stream = m.add_stream_encoder(enc)?;
                var_map.insert(var.id(), (*stream).index);
            }
            m.open(None)?;
            m
        };
        Ok(Self {
            id: *id,
            muxer,
            var_map,
        })
    }
}

impl Egress for RecorderEgress {
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        if let Some(stream) = self.var_map.get(variant) {
            // very important for muxer to know which stream this pkt belongs to
            (*packet).stream_index = *stream;

            self.muxer.write_packet(packet)?;
        }
        Ok(EgressResult::None)
    }

    unsafe fn reset(&mut self) -> Result<()> {
        self.muxer.close()
    }
}
