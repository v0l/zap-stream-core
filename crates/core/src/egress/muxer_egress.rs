use anyhow::{Result, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AVFormatContext, AVStream, avcodec_parameters_copy, avformat_alloc_context,
    avformat_free_context, avformat_new_stream,
};
use ffmpeg_rs_raw::{AvPacketRef, Muxer};
use std::collections::HashMap;
use std::ptr;
use std::time::{Duration, Instant};
use tracing::warn;
use uuid::Uuid;

use crate::egress::{Egress, EgressResult, EncoderOrSourceStream, EncoderVariantGroup};
use crate::metrics::PacketMetrics;

/// Snapshot of the muxer's stream layout used to rebuild the context on reconnect.
///
/// The streams live inside a dedicated `AVFormatContext` owned by this struct, so no
/// pointers into other components (encoders owned by worker threads, demuxer streams)
/// are retained. This avoids the dangling-pointer hazard of referencing objects that
/// may move or be freed while the egress is still alive.
struct ReconnectTemplate {
    ctx: *mut AVFormatContext,
    /// Ordered (variant id, template stream index) pairs
    streams: Vec<(Uuid, usize)>,
}

// SAFETY: the template context is exclusively owned by MuxerEgress and only accessed
// from the thread currently driving the egress (guarded externally by the egress mutex).
unsafe impl Send for ReconnectTemplate {}

impl ReconnectTemplate {
    fn new() -> Result<Self> {
        let ctx = unsafe { avformat_alloc_context() };
        if ctx.is_null() {
            bail!("Failed to allocate reconnect template context");
        }
        Ok(Self {
            ctx,
            streams: Vec::new(),
        })
    }

    /// Snapshot a stream that was just added to the muxer
    unsafe fn add_stream(&mut self, var_id: Uuid, src: *mut AVStream) -> Result<()> {
        unsafe {
            let stream = avformat_new_stream(self.ctx, ptr::null_mut());
            if stream.is_null() {
                bail!("Failed to allocate reconnect template stream");
            }
            let ret = avcodec_parameters_copy((*stream).codecpar, (*src).codecpar);
            if ret < 0 {
                bail!("Failed to copy codec parameters to reconnect template");
            }
            (*stream).time_base = (*src).time_base;
            (*stream).sample_aspect_ratio = (*src).sample_aspect_ratio;
            self.streams.push((var_id, (*stream).index as usize));
            Ok(())
        }
    }

    fn get_stream(&self, idx: usize) -> *mut AVStream {
        unsafe { *(*self.ctx).streams.add(idx) }
    }
}

impl Drop for ReconnectTemplate {
    fn drop(&mut self) {
        unsafe {
            if !self.ctx.is_null() {
                avformat_free_context(self.ctx);
                self.ctx = ptr::null_mut();
            }
        }
    }
}

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
    /// Reconnect info: owned snapshot of the stream layout.
    /// None means reconnect is not supported for this egress.
    reconnect: Option<ReconnectTemplate>,
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
        let mut reconnect = if reconnectable {
            Some(ReconnectTemplate::new()?)
        } else {
            None
        };

        let muxer = unsafe {
            for g in &group.streams {
                let stream = match g.stream {
                    EncoderOrSourceStream::Encoder(enc) => muxer.add_stream_encoder(enc)?,
                    EncoderOrSourceStream::SourceStream(s) => muxer.add_copy_stream(s)?,
                };
                (*(*stream).codecpar).codec_tag = 0;
                var_map.insert(g.variant.id(), (*stream).index);
                if let Some(t) = reconnect.as_mut() {
                    t.add_stream(g.variant.id(), stream)?;
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
            reconnect,
            last_failure: None,
        })
    }

    /// Try to reconnect the muxer after a write failure.
    /// Returns true if reconnection succeeded.
    unsafe fn try_reconnect(&mut self) -> bool {
        let Some(ref template) = self.reconnect else {
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

            // re-add all streams in original order from the owned template snapshot
            let mut new_map = HashMap::new();
            for (var_id, template_idx) in &template.streams {
                match self.muxer.add_copy_stream(template.get_stream(*template_idx)) {
                    Ok(stream) => {
                        (*(*stream).codecpar).codec_tag = 0;
                        new_map.insert(*var_id, (*stream).index);
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
            self.var_map = new_map;
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
                if self.reconnect.is_some() {
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
