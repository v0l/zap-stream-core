use crate::ingress::{ConnectionInfo, spawn_pipeline};
use crate::overseer::Overseer;
use anyhow::Result;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tracing::info;
use uuid::Uuid;

pub async fn listen(
    out_dir: String,
    addr: String,
    overseer: Arc<dyn Overseer>,
    shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("TCP listening on: {}", &addr);
    let l_shutdown = shutdown_rx.subscribe();
    loop {
        tokio::select! {
            _ = l_shutdown.recv() => {
                break;
            }
            Ok((socket, ip)) = listener.accept() => {
                let info = ConnectionInfo {
                    id: Uuid::new_v4(),
                    ip_addr: ip.to_string(),
                    endpoint: "tcp",
                    app_name: "".to_string(),
                    key: "test".to_string(),
                };
                let socket = socket.into_std()?;
                socket.set_nonblocking(false)?;
                spawn_pipeline(
                    Handle::current(),
                    info,
                    out_dir.clone(),
                    overseer.clone(),
                    Box::new(socket),
                    None,
                    None,
                );
            }
        }
    }

    info!("TCP listener closed.");
    Ok(())
}
