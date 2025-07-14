use crate::overseer::Overseer;
use crate::pipeline::runner::{PipelineCommand, PipelineRunner};
use crate::metrics::PacketMetrics;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::Arc;
use std::time::Instant;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

pub mod file;
#[cfg(feature = "rtmp")]
pub mod rtmp;
#[cfg(feature = "srt")]
pub mod srt;
pub mod tcp;
pub mod test;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Unique ID of this connection / pipeline
    pub id: Uuid,

    /// Name of the ingest point
    pub endpoint: &'static str,

    /// IP address of the connection
    pub ip_addr: String,

    /// App name, empty unless RTMP ingress
    pub app_name: String,

    /// Stream key
    pub key: String,
}

pub fn spawn_pipeline(
    handle: Handle,
    info: ConnectionInfo,
    out_dir: String,
    seer: Arc<dyn Overseer>,
    reader: Box<dyn Read + Send>,
    url: Option<String>,
    rx: Option<UnboundedReceiver<PipelineCommand>>,
) {
    match PipelineRunner::new(handle, out_dir, seer, info, reader, url, rx) {
        Ok(pl) => match run_pipeline(pl) {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to run PipelineRunner: {}", e);
            }
        },
        Err(e) => {
            error!("Failed to create PipelineRunner: {}", e);
        }
    }
}

pub fn run_pipeline(mut pl: PipelineRunner) -> anyhow::Result<()> {
    info!("New client connected: {}", &pl.connection.ip_addr);

    std::thread::Builder::new()
        .name(format!(
            "client:{}:{}",
            pl.connection.endpoint, pl.connection.id
        ))
        .spawn(move || {
            pl.run();
        })?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct IngressStats {
    pub bitrate: usize,
}

#[derive(Clone, Debug)]
pub struct EgressStats {
    pub bitrate: usize,
}

/// Common buffered reader functionality for ingress sources
pub struct BufferedReader {
    pub buf: Vec<u8>,
    pub max_buffer_size: usize,
    pub last_buffer_log: Instant,
    pub metrics: PacketMetrics,
}

impl BufferedReader {
    pub fn new(
        capacity: usize,
        max_size: usize,
        source_name: &'static str,
        metrics_sender: Option<UnboundedSender<PipelineCommand>>,
    ) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            max_buffer_size: max_size,
            last_buffer_log: Instant::now(),
            metrics: PacketMetrics::new(source_name, metrics_sender),
        }
    }

    /// Add data to buffer with size limit and performance tracking
    pub fn add_data(&mut self, data: &[u8]) {
        // Inline buffer management to avoid borrow issues
        if self.buf.len() + data.len() > self.max_buffer_size {
            let bytes_to_drop = (self.buf.len() + data.len()) - self.max_buffer_size;
            warn!(
                "{} buffer full ({} bytes), dropping {} oldest bytes",
                self.metrics.source_name,
                self.buf.len(),
                bytes_to_drop
            );
            self.buf.drain(..bytes_to_drop);
        }
        self.buf.extend(data);

        // Update performance counters using PacketMetrics (auto-reports when interval elapsed)
        self.metrics.update(data.len());
    }

    /// Read data from buffer
    pub fn read_buffered(&mut self, buf: &mut [u8]) -> usize {
        let to_drain = buf.len().min(self.buf.len());
        if to_drain > 0 {
            let drain = self.buf.drain(..to_drain);
            buf[..to_drain].copy_from_slice(drain.as_slice());
        }
        to_drain
    }
}
