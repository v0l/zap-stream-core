use crate::ingress::{BufferedReader, ConnectionInfo, setup_term_handler, spawn_pipeline};
use crate::overseer::{ConnectResult, Overseer};
use crate::pipeline::runner::PipelineCommand;
use anyhow::Result;
use futures_util::StreamExt;
use futures_util::stream::FusedStream;
use srt_tokio::{SrtListener, SrtSocket};
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
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

    let out_dir = PathBuf::from(out_dir);
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
                let mut br = BufferedReader::new(4096, MAX_SRT_BUFFER_SIZE, "SRT", Some(tx.clone()));
                match overseer.connect(&info).await {
                    Ok(ConnectResult::Allow { enable_stream_dump }) => {
                        if enable_stream_dump {
                            // Dump raw SRT stream for debugging
                            let dump_stream = File::create(out_dir.join("stream.dump"))?;
                            br.set_dump_handle(dump_stream);
                        }
                    }
                    Ok(ConnectResult::Deny { reason }) => {
                        warn!("Connection denied: {reason}");
                        return Ok(());
                    }
                    Err(e) => {
                        error!("Failed to handle connect request: {}", e);
                    }
                }

                setup_term_handler(shutdown.clone(), tx.clone());
                let out_dir = out_dir.join(info.id.to_string());
                if !out_dir.exists() {
                    std::fs::create_dir_all(&out_dir)?;
                }

                // Dump raw SRT stream for debugging
                let dump_stream = File::create(out_dir.join("stream.dump"))?;
                br.set_dump_handle(dump_stream);

                // spawn pipeline runner thread
                if let Err(e) = spawn_pipeline(
                    Handle::current(),
                    info,
                    out_dir,
                    overseer.clone(),
                    Box::new(SrtReader {
                        handle: Handle::current(),
                        socket,
                        buffer: br,
                        tx,
                    }),
                    None,
                    Some(rx),
                ) {
                    error!("Failed to spawn pipeline: {}", e);
                }
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
