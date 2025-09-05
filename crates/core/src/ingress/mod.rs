use crate::metrics::PacketMetrics;
use crate::overseer::Overseer;
use crate::pipeline::runner::{PipelineCommand, PipelineRunner};
use anyhow::Result;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod file;
#[cfg(feature = "ingress-rtmp")]
pub mod rtmp;
#[cfg(feature = "ingress-srt")]
pub mod srt;
#[cfg(feature = "ingress-tcp")]
pub mod tcp;
#[cfg(feature = "ingress-test")]
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
) -> Result<JoinHandle<()>> {
    let pl = PipelineRunner::new(handle, out_dir, seer, info, reader, url, rx)?;
    run_pipeline(pl)
}

pub fn run_pipeline(mut pl: PipelineRunner) -> Result<JoinHandle<()>> {
    info!("New client connected: {}", &pl.connection.ip_addr);

    Ok(std::thread::Builder::new()
        .name(format!(
            "client:{}:{}",
            pl.connection.endpoint, pl.connection.id
        ))
        .spawn(move || {
            pl.run();
            info!("Pipeline {} completed.", pl.connection.id);
        })?)
}

pub(crate) fn setup_term_handler(
    shutdown: CancellationToken,
    tx: UnboundedSender<PipelineCommand>,
) {
    // handle termination
    tokio::spawn(async move {
        shutdown.cancelled().await;
        if let Err(e) = tx.send(PipelineCommand::Shutdown) {
            warn!("Failed to send shutdown signal: {}", e);
        }
    });
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointStats {
    pub name: String,
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
