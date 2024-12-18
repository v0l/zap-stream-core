use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use log::info;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("TCP listening on: {}", &addr);
    while let Ok((socket, ip)) = listener.accept().await {
        let info = ConnectionInfo {
            ip_addr: ip.to_string(),
            endpoint: addr.clone(),
            app_name: "".to_string(),
            key: "no-key-tcp".to_string(),
        };
        let socket = socket.into_std()?;
        spawn_pipeline(
            Handle::current(),
            info,
            out_dir.clone(),
            overseer.clone(),
            Box::new(socket),
        );
    }
    Ok(())
}
