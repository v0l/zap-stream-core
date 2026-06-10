use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVStream;
use ffmpeg_rs_raw::{AvPacketRef, Encoder, Muxer};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::warn;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult, EncoderOrSourceStream, EncoderVariantGroup};
use crate::metrics::PacketMetrics;

/// A stream entry stored for muxer reconnection
enum ReconnectStream {
    Encoder(*const Encoder),
    SourceStream(*mut AVStream),
}

unsafe impl Send for ReconnectStream {}

/// Generic muxer egress which accepts a pre-build muxer instance
pub struct MuxerEgress {
    /// Internal muxer writing the output packets
    muxer: Muxer,
    /// Mapping from Variant ID to stream index
    var_map: HashMap<Uuid, i32>,
    /// Packet metrics tracking
    metrics: PacketMetrics,
    /// If packet muxing fails should the pipeline also fail
    critical: bool,
    /// Reconnect info: ordered list of streams to re-add after reinit, keyed by variant id.
    /// None means reconnect is not supported for this egress.
    reconnect_streams: Option<Vec<(Uuid, ReconnectStream)>>,
    /// Backoff: don't hammer a failing endpoint
    last_failure: Option<Instant>,
}

impl MuxerEgress {
    pub fn new(
        name: &str,
        muxer: Muxer,
        group: &EncoderVariantGroup,
        options: Option<HashMap<String, String>>,
        critical: bool,
    ) -> Result<Self> {
        Self::new_inner(name, muxer, group, options, critical, false)
    }

    /// Create an RTMP-forward egress that will automatically reconnect on write failure.
    pub fn new_rtmp_forward(name: &str, muxer: Muxer, group: &EncoderVariantGroup) -> Result<Self> {
        Self::new_inner(name, muxer, group, None, false, true)
    }

    fn new_inner(
        name: &str,
        mut muxer: Muxer,
        group: &EncoderVariantGroup,
        options: Option<HashMap<String, String>>,
        critical: bool,
        reconnectable: bool,
    ) -> Result<Self> {
        let mut var_map = HashMap::new();
        let mut reconnect_streams: Vec<(Uuid, ReconnectStream)> = Vec::new();

        let muxer = unsafe {
            for g in &group.streams {
                match g.stream {
                    EncoderOrSourceStream::Encoder(enc) => {
                        let stream = muxer.add_stream_encoder(enc)?;
                        (*(*stream).codecpar).codec_tag = 0;
                        var_map.insert(g.variant.id(), (*stream).index);
                        if reconnectable {
                            reconnect_streams.push((g.variant.id(), ReconnectStream::Encoder(enc)));
                        }
                    }
                    EncoderOrSourceStream::SourceStream(stream) => {
                        let stream = muxer.add_copy_stream(stream)?;
                        (*(*stream).codecpar).codec_tag = 0;
                        var_map.insert(g.variant.id(), (*stream).index);
                        if reconnectable {
                            reconnect_streams
                                .push((g.variant.id(), ReconnectStream::SourceStream(stream)));
                        }
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
            critical,
            reconnect_streams: if reconnectable {
                Some(reconnect_streams)
            } else {
                None
            },
            last_failure: None,
        })
    }

    /// Try to reconnect the muxer after a write failure.
    /// Returns true if reconnection succeeded.
    unsafe fn try_reconnect(&mut self) -> bool {
        let Some(ref streams) = self.reconnect_streams else {
            return false;
        };

        // close the broken connection (ignore trailer-write errors)
        unsafe {
            let _ = self.muxer.close();

            // re-init the context with the same URL/format
            if let Err(e) = self.muxer.init() {
                warn!("RTMP reconnect: init failed: {}", e);
                return false;
            }

            // re-add all streams in original order
            self.var_map.clear();
            for (var_id, rs) in streams {
                let result = match rs {
                    ReconnectStream::Encoder(enc) => self.muxer.add_stream_encoder(&**enc),
                    ReconnectStream::SourceStream(src) => self.muxer.add_copy_stream(*src),
                };
                match result {
                    Ok(stream) => {
                        (*(*stream).codecpar).codec_tag = 0;
                        self.var_map.insert(*var_id, (*stream).index);
                    }
                    Err(e) => {
                        warn!("RTMP reconnect: add_stream failed: {}", e);
                        return false;
                    }
                }
            }

            if let Err(e) = self.muxer.open(None) {
                warn!("RTMP reconnect: open failed: {}", e);
                return false;
            }
        }

        true
    }
}

impl Egress for MuxerEgress {
    fn process_pkt(&mut self, mut packet: AvPacketRef, variant: &Uuid) -> Result<EgressResult> {
        // Copy the stream index out so we don't hold an immutable borrow into self
        // while potentially calling try_reconnect (which needs &mut self).
        let Some(stream_index) = self.var_map.get(variant).copied() else {
            return Ok(EgressResult::None);
        };

        // Skip packets during reconnect backoff (5 seconds)
        if let Some(t) = self.last_failure {
            if t.elapsed() < Duration::from_secs(5) {
                return Ok(EgressResult::None);
            }
            // Backoff expired — attempt reconnect
            warn!("Attempting RTMP reconnect for {}", self.metrics.source_name);
            let ok = unsafe { self.try_reconnect() };
            if ok {
                self.last_failure = None;
                warn!("RTMP reconnect succeeded for {}", self.metrics.source_name);
            } else {
                self.last_failure = Some(Instant::now());
                return Ok(EgressResult::None);
            }
        }

        // Update metrics with packet data (auto-reports when interval elapsed)
        self.metrics.update(packet.size as usize);

        // very important for muxer to know which stream this pkt belongs to
        packet.stream_index = stream_index;
        if let Err(e) = self.muxer.write_packet(&packet) {
            if self.critical {
                return Err(e);
            } else {
                warn!("Error muxing packet in {}: {}", self.metrics.source_name, e);
                if self.reconnect_streams.is_some() {
                    self.last_failure = Some(Instant::now());
                }
            }
        };

        Ok(EgressResult::None)
    }

    fn reset(&mut self) -> Result<EgressResult> {
        unsafe {
            self.muxer.close()?;
            Ok(EgressResult::None)
        }
    }
}
