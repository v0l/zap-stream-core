use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use log::info;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use uuid::Uuid;

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("TCP listening on: {}", &addr);
    while let Ok((socket, ip)) = listener.accept().await {
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
    Ok(())
}
