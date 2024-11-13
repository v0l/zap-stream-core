use anyhow::Result;
use log::info;
use tokio::net::TcpListener;

use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::settings::Settings;

pub async fn listen(addr: String, settings: Settings) -> Result<()> {
    let listener = TcpListener::bind(addr.clone()).await?;

    info!("TCP listening on: {}", addr.clone());
    while let Ok((socket, ip)) = listener.accept().await {
        let info = ConnectionInfo {
            ip_addr: ip.to_string(),
            endpoint: addr.clone(),
        };
        let socket = socket.into_std()?;
        spawn_pipeline(info, settings.clone(), Box::new(socket));
    }
    Ok(())
}
