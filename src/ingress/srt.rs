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

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let (_binding, mut packets) = SrtListener::builder().bind(&addr).await?;

    info!("SRT listening on: {}", &addr);
    while let Some(request) = packets.incoming().next().await {
        let mut socket = request.accept(None).await?;
        let info = ConnectionInfo {
            endpoint: addr.clone(),
            ip_addr: socket.settings().remote.to_string(),
            key: "".to_string(),
        };
        spawn_pipeline(info, out_dir.clone(), overseer.clone(), Box::new(socket)).await;
    }
    Ok(())
}
