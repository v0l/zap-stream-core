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
            }),
        );
    }
    Ok(())
}

struct SrtReader {
    pub handle: Handle,
    pub socket: SrtSocket,
    pub buf: Vec<u8>,
}

impl SrtReader {
    /// Add data to buffer with size limit to prevent unbounded growth
    fn add_to_buffer(&mut self, data: &[u8]) {
        if self.buf.len() + data.len() > MAX_SRT_BUFFER_SIZE {
            let bytes_to_drop = (self.buf.len() + data.len()) - MAX_SRT_BUFFER_SIZE;
            warn!("SRT buffer full ({} bytes), dropping {} oldest bytes", 
                  self.buf.len(), bytes_to_drop);
            self.buf.drain(..bytes_to_drop);
        }
        self.buf.extend(data);
    }
}

impl Read for SrtReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let (mut rx, _) = self.socket.split_mut();
        while self.buf.len() < buf.len() {
            if rx.is_terminated() {
                return Ok(0);
            }
            if let Some((_, data)) = self.handle.block_on(rx.next()) {
                self.add_to_buffer(data.iter().as_slice());
            }
        }
        let drain = self.buf.drain(..buf.len());
        buf.copy_from_slice(drain.as_slice());
        Ok(buf.len())
    }
}
