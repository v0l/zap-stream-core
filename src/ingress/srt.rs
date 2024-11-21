use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use crate::pipeline::runner::PipelineRunner;
use crate::settings::Settings;
use anyhow::Result;
use futures_util::stream::FusedStream;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use log::{error, info, warn};
use srt_tokio::{SrtListener, SrtSocket};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc::unbounded_channel;
use warp::Buf;

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let binder: SocketAddr = addr.parse()?;
    let (_binding, mut packets) = SrtListener::builder().bind(binder).await?;

    info!("SRT listening on: {}", &addr);
    while let Some(request) = packets.incoming().next().await {
        let mut socket = request.accept(None).await?;
        let info = ConnectionInfo {
            endpoint: addr.clone(),
            ip_addr: socket.settings().remote.to_string(),
            key: socket
                .settings()
                .stream_id
                .as_ref()
                .map_or(String::new(), |s| s.to_string()),
        };
        spawn_pipeline(
            info,
            out_dir.clone(),
            overseer.clone(),
            Box::new(SrtReader {
                handle: Handle::current(),
                socket,
                buf: Vec::with_capacity(4096),
            }),
        )
        .await;
    }
    Ok(())
}

struct SrtReader {
    pub handle: Handle,
    pub socket: SrtSocket,
    pub buf: Vec<u8>,
}

impl Read for SrtReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let (mut rx, _) = self.socket.split_mut();
        while self.buf.len() < buf.len() {
            if rx.is_terminated() {
                return Ok(0);
            }
            if let Some((_, mut data)) = self.handle.block_on(rx.next()) {
                self.buf.extend(data.iter().as_slice());
            }
        }
        let drain = self.buf.drain(..buf.len());
        buf.copy_from_slice(drain.as_slice());
        Ok(buf.len())
    }
}
