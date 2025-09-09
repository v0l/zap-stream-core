use crate::ingress::{BufferedReader, ConnectionInfo, setup_term_handler, spawn_pipeline};
use crate::overseer::Overseer;
use crate::pipeline::runner::PipelineCommand;
use anyhow::Result;
use futures_util::StreamExt;
use futures_util::stream::FusedStream;
use tracing::info;
use srt_tokio::{SrtListener, SrtSocket};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_SRT_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit

pub async fn listen(
    out_dir: String,
    addr: String,
    overseer: Arc<dyn Overseer>,
    shutdown: CancellationToken,
) -> Result<()> {
    let binder: SocketAddr = addr.parse()?;
    let (_binding, mut packets) = SrtListener::builder().bind(binder).await?;

    info!("SRT listening on: {}", &addr);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                break;
            }
            Some(request) = packets.incoming().next() => {
                let socket = request.accept(None).await?;
                let info = ConnectionInfo {
                    id: Uuid::new_v4(),
                    endpoint: "srt",
                    ip_addr: socket.settings().remote.to_string(),
                    app_name: "".to_string(),
                    key: socket
                        .settings()
                        .stream_id
                        .as_ref()
                        .map_or(String::new(), |s| s.to_string()),
                };
                let (tx, rx) = unbounded_channel();
                setup_term_handler(shutdown.clone(), tx.clone());

                // spawn pipeline runner thread
                spawn_pipeline(
                    Handle::current(),
                    info,
                    out_dir.clone(),
                    overseer.clone(),
                    Box::new(SrtReader {
                        handle: Handle::current(),
                        socket,
                        buffer: BufferedReader::new(4096, MAX_SRT_BUFFER_SIZE, "SRT", Some(tx.clone())),
                        tx,
                    }),
                    None,
                    Some(rx),
                );
            }
        }
    }

    info!("SRT listener {} shutdown.", &addr);
    Ok(())
}

struct SrtReader {
    pub handle: Handle,
    pub socket: SrtSocket,
    pub buffer: BufferedReader,
    pub tx: UnboundedSender<PipelineCommand>, // TODO: implement clean shutdown
}

impl Read for SrtReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let (mut rx, _) = self.socket.split_mut();
        while self.buffer.buf.len() < buf.len() {
            if rx.is_terminated() {
                return Ok(0);
            }
            if let Some((_, data)) = self.handle.block_on(rx.next()) {
                let data_slice = data.iter().as_slice();
                self.buffer.add_data(data_slice);
            }
        }
        Ok(self.buffer.read_buffered(buf))
    }
}
