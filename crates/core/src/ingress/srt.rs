use crate::endpoint::EndpointConfigurator;
use crate::ingress::{BufferedReader, ConnectionInfo, setup_term_handler, spawn_pipeline};
use crate::overseer::{ConnectResult, Overseer};
use crate::pipeline::PipelineCommand;
use anyhow::{Result, anyhow};
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
use url::Url;
use uuid::Uuid;

const MAX_SRT_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit

pub async fn listen(
    out_dir: String,
    addr: Url,
    overseer: Arc<dyn Overseer>,
    endpoint_config: Arc<dyn EndpointConfigurator>,
    shutdown: CancellationToken,
) -> Result<()> {
    let binder = addr
        .socket_addrs(|| Some(3333))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Could not parse bind address from {}", addr))?;
    let (_binding, mut packets) = SrtListener::builder().bind(binder).await?;

    let out_dir = PathBuf::from(out_dir);
    info!("SRT listening on: {}", &addr);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                break;
            }
            Some(request) = packets.incoming().next() => {
                let socket = match request.accept(None).await {
                    Ok(s) => s,
                    Err(e) => {
                        // a single failed handshake must not kill the listener
                        warn!("Failed to accept SRT connection: {}", e);
                        continue;
                    }
                };
                let mut info = ConnectionInfo {
                    id: Uuid::new_v4(),
                    endpoint: "srt".to_string(),
                    ip_addr: socket.settings().remote.to_string(),
                    app_name: "".to_string(),
                    key: socket
                        .settings()
                        .stream_id
                        .as_ref()
                        .map_or(String::new(), |s| s.to_string()),
                };
                let (tx, rx) = unbounded_channel();
                let mut dump_stream = false;
                match overseer.connect(&info).await {
                    Ok(ConnectResult::Allow { enable_stream_dump, stream_id_override }) => {
                        if let Some(id) = stream_id_override {
                            info.id = id;
                        }
                        dump_stream = enable_stream_dump;
                    }
                    Ok(ConnectResult::Deny { reason }) => {
                        // reject only this connection; returning here would shut down
                        // the whole SRT listener for a single bad stream key
                        warn!("Connection denied: {reason}");
                        continue;
                    }
                    Err(e) => {
                        // do NOT spawn a pipeline for a connection we could not
                        // authenticate/authorize
                        error!("Failed to handle connect request: {}", e);
                        continue;
                    }
                }

                let mtx = BufferedReader::stats_to_overseer(info.id.clone(), &Handle::current(), overseer.clone());
                let mut br = BufferedReader::new(4096, MAX_SRT_BUFFER_SIZE, "SRT", Some(mtx));
                setup_term_handler(shutdown.clone(), tx.clone());
                let out_dir = out_dir.join(info.id.to_string());
                if !out_dir.exists()
                    && let Err(e) = std::fs::create_dir_all(&out_dir)
                {
                    // per-connection failure must not kill the listener
                    error!("Failed to create output directory {}: {}", out_dir.display(), e);
                    continue;
                }

                // Dump raw SRT stream for debugging (best-effort)
                if dump_stream {
                    match File::create(out_dir.join("stream.dump")) {
                        Ok(h) => br.set_dump_handle(h),
                        Err(e) => warn!("Failed to create stream dump file: {}", e),
                    }
                }

                // spawn pipeline runner thread
                if let Err(e) = spawn_pipeline(
                    Handle::current(),
                    info,
                    out_dir,
                    overseer.clone(),
                    endpoint_config.clone(),
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
                // stream ended: return any remaining buffered data before EOF
                return Ok(self.buffer.read_buffered(buf));
            }
            if let Some((_, data)) = self.handle.block_on(rx.next()) {
                let data_slice = data.iter().as_slice();
                self.buffer.add_data(data_slice);
            }
        }
        Ok(self.buffer.read_buffered(buf))
    }
}
