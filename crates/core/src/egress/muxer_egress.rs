use anyhow::Result;
use ffmpeg_rs_raw::{AvPacketRef, Muxer};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult, EncoderOrSourceStream, EncoderVariantGroup};
use crate::metrics::PacketMetrics;

/// Generic muxer egress which accepts a pre-build muxer instance
pub struct MuxerEgress {
    /// Internal muxer writing the output packets
    muxer: Muxer,
    /// Mapping from Variant ID to stream index
    var_map: HashMap<Uuid, i32>,
    /// Packet metrics tracking
    metrics: PacketMetrics,
}

impl MuxerEgress {
    pub fn new(name: &str, mut muxer: Muxer, group: &EncoderVariantGroup, options: Option<HashMap<String, String>>) -> Result<Self> {
        let mut var_map = HashMap::new();
        let muxer = unsafe {
            for g in &group.streams {
                match g.stream {
                    EncoderOrSourceStream::Encoder(enc) => {
                        let stream = muxer.add_stream_encoder(enc)?;
                        (*(*stream).codecpar).codec_tag = 0;
                        var_map.insert(g.variant.id(), (*stream).index);
                    }
                    EncoderOrSourceStream::SourceStream(stream) => {
                        let stream = muxer.add_copy_stream(stream)?;
                        (*(*stream).codecpar).codec_tag = 0;
                        var_map.insert(g.variant.id(), (*stream).index);
                    }
                }
            }
            muxer.open(options)?;
            muxer
        };
        Ok(Self {
            muxer,
            var_map,
            metrics: PacketMetrics::new(name, None),
        })
    }
}

impl Egress for MuxerEgress {
    fn process_pkt(&mut self, mut packet: AvPacketRef, variant: &Uuid) -> Result<EgressResult> {
        if let Some(stream) = self.var_map.get(variant) {
            // Update metrics with packet data (auto-reports when interval elapsed)
            self.metrics.update(packet.size as usize);

            // very important for muxer to know which stream this pkt belongs to
            packet.stream_index = *stream;
            self.muxer.write_packet(&packet)?;
        }
        Ok(EgressResult::None)
    }

    fn reset(&mut self) -> Result<EgressResult> {
        unsafe {
            self.muxer.close()?;
            Ok(EgressResult::None)
        }
    }
}
