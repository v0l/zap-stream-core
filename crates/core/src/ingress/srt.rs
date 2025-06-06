use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use futures_util::stream::FusedStream;
use futures_util::StreamExt;
use log::{info, warn};
use srt_tokio::{SrtListener, SrtSocket};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::runtime::Handle;

const MAX_SRT_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let binder: SocketAddr = addr.parse()?;
    let (_binding, mut packets) = SrtListener::builder().bind(binder).await?;

    info!("SRT listening on: {}", &addr);
    while let Some(request) = packets.incoming().next().await {
        let socket = request.accept(None).await?;
        let info = ConnectionInfo {
            endpoint: addr.clone(),
            ip_addr: socket.settings().remote.to_string(),
            app_name: "".to_string(),
            key: socket
                .settings()
                .stream_id
                .as_ref()
                .map_or(String::new(), |s| s.to_string()),
        };
        spawn_pipeline(
            Handle::current(),
            info,
            out_dir.clone(),
            overseer.clone(),
            Box::new(SrtReader {
                handle: Handle::current(),
                socket,
                buf: Vec::with_capacity(4096),
                last_buffer_log: Instant::now(),
                bytes_processed: 0,
                packets_received: 0,
            }),
        );
    }
    Ok(())
}

struct SrtReader {
    pub handle: Handle,
    pub socket: SrtSocket,
    pub buf: Vec<u8>,
    last_buffer_log: Instant,
    bytes_processed: u64,
    packets_received: u64,
}

impl Read for SrtReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let (mut rx, _) = self.socket.split_mut();
        while self.buf.len() < buf.len() {
            if rx.is_terminated() {
                return Ok(0);
            }
            if let Some((_, data)) = self.handle.block_on(rx.next()) {
                let data_slice = data.iter().as_slice();
                
                // Inline buffer management to avoid borrow issues
                if self.buf.len() + data_slice.len() > MAX_SRT_BUFFER_SIZE {
                    let bytes_to_drop = (self.buf.len() + data_slice.len()) - MAX_SRT_BUFFER_SIZE;
                    warn!("SRT buffer full ({} bytes), dropping {} oldest bytes", 
                          self.buf.len(), bytes_to_drop);
                    self.buf.drain(..bytes_to_drop);
                }
                self.buf.extend(data_slice);
                
                // Update performance counters
                self.bytes_processed += data_slice.len() as u64;
                self.packets_received += 1;
                
                // Log buffer status every 5 seconds
                if self.last_buffer_log.elapsed().as_secs() >= 5 {
                    let buffer_util = (self.buf.len() as f32 / MAX_SRT_BUFFER_SIZE as f32) * 100.0;
                    let elapsed = self.last_buffer_log.elapsed();
                    let mbps = (self.bytes_processed as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000.0);
                    let pps = self.packets_received as f64 / elapsed.as_secs_f64();
                    
                    info!(
                        "SRT ingress: {:.1} Mbps, {:.1} packets/sec, buffer: {}% ({}/{} bytes)",
                        mbps, pps, buffer_util as u32, self.buf.len(), MAX_SRT_BUFFER_SIZE
                    );
                    
                    // Reset counters
                    self.last_buffer_log = Instant::now();
                    self.bytes_processed = 0;
                    self.packets_received = 0;
                }
            }
        }
        let drain = self.buf.drain(..buf.len());
        buf.copy_from_slice(drain.as_slice());
        Ok(buf.len())
    }
}
