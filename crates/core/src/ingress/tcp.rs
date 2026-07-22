use crate::endpoint::EndpointConfigurator;
use crate::ingress::{ConnectionInfo, setup_term_handler, spawn_pipeline};
use crate::overseer::Overseer;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

pub async fn listen(
    out_dir: String,
    addr: SocketAddr,
    overseer: Arc<dyn Overseer>,
    endpoint_config: Arc<dyn EndpointConfigurator>,
    shutdown_rx: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("TCP listening on: {}", &addr);
    let l_shutdown = shutdown_rx.clone();
    loop {
        tokio::select! {
            _ = l_shutdown.cancelled() => {
                break;
            }
            Ok((socket, ip)) = listener.accept() => {
                let info = ConnectionInfo {
                    id: Uuid::new_v4(),
                    ip_addr: ip.to_string(),
                    endpoint: "tcp".to_string(),
                    app_name: "".to_string(),
                    key: "test".to_string(),
                };
                let out_dir = PathBuf::from(&out_dir).join(info.id.to_string());
                if !out_dir.exists() {
                    std::fs::create_dir_all(&out_dir)?;
                }
                let socket = socket.into_std()?;
                socket.set_nonblocking(false)?;
                // wire up shutdown so TCP pipelines also terminate cleanly on ctrl-c
                let (tx, rx) = unbounded_channel();
                setup_term_handler(shutdown_rx.clone(), tx);
                if let Err(e) = spawn_pipeline(
                    Handle::current(),
                    info,
                    out_dir,
                    overseer.clone(),
                    endpoint_config.clone(),
                    Box::new(socket),
                    None,
                    Some(rx),
                ) {
                    error!("Failed to spawn pipeline: {}", e);
                }
            }
        }
    }

    info!("TCP listener closed.");
    Ok(())
}
