use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use crate::pipeline::runner::PipelineRunner;
use crate::settings::Settings;
use anyhow::Result;
use futures_util::{StreamExt, TryStreamExt};
use log::{error, info, warn};
use srt_tokio::{SrtListener, SrtSocket};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

pub async fn listen(listen_addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let (_binding, mut packets) = SrtListener::builder().bind(listen_addr.clone()).await?;

    info!("SRT listening on: {}", listen_addr.clone());
    while let Some(request) = packets.incoming().next().await {
        let mut socket = request.accept(None).await?;
        let info = ConnectionInfo {
            endpoint: listen_addr.clone(),
            ip_addr: socket.settings().remote.to_string(),
        };
        spawn_pipeline(info, overseer.clone(), Box::new(socket)).await;
    }
    Ok(())
}
