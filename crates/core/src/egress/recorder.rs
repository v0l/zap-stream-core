use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPacket;
use ffmpeg_rs_raw::Muxer;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult, EncoderOrSourceStream};
use crate::variant::{StreamMapping, VariantStream};

pub struct RecorderEgress {
    /// Internal muxer writing the output packets
    muxer: Muxer,
    /// Mapping from Variant ID to stream index
    var_map: HashMap<Uuid, i32>,
}

impl RecorderEgress {
    pub const FILENAME: &'static str = "recording.mp4";

    pub fn new<'a>(
        out_dir: PathBuf,
        variants: impl Iterator<Item = (&'a VariantStream, EncoderOrSourceStream<'a>)>,
    ) -> Result<Self> {
        let out_file = out_dir.join(Self::FILENAME);
        let mut var_map = HashMap::new();
        let muxer = unsafe {
            let mut m = Muxer::builder()
                .with_output_path(out_file.to_str().unwrap(), None)?
                .build()?;
            for (var, enc) in variants {
                match enc {
                    EncoderOrSourceStream::Encoder(enc) => {
                        let stream = m.add_stream_encoder(enc)?;
                        var_map.insert(var.id(), (*stream).index);
                    }
                    EncoderOrSourceStream::SourceStream(stream) => {
                        let stream = m.add_copy_stream(stream)?;
                        var_map.insert(var.id(), (*stream).index);
                    }
                }
            }
            let mut options = HashMap::new();
            options.insert("movflags".to_string(), "faststart".to_string());

            m.open(Some(options))?;
            m
        };
        Ok(Self { muxer, var_map })
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
