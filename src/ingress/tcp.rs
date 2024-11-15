use anyhow::Result;
use log::info;
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;

pub async fn listen(addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let listener = TcpListener::bind(addr.clone()).await?;

    info!("TCP listening on: {}", addr.clone());
    while let Ok((socket, ip)) = listener.accept().await {
        let info = ConnectionInfo {
            ip_addr: ip.to_string(),
            endpoint: addr.clone(),
            key: "".to_string(),
        };
        let socket = socket.into_std()?;
        spawn_pipeline(info, overseer.clone(), Box::new(socket)).await;
    }
    Ok(())
}
