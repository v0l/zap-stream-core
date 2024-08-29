use futures_util::{StreamExt, TryStreamExt};
use log::{error, info, warn};
use srt_tokio::SrtListener;
use tokio::sync::mpsc::unbounded_channel;

use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;

pub async fn listen(addr: String, builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    let (_binding, mut packets) = SrtListener::builder().bind(addr.clone()).await?;

    info!("SRT listening on: {}", addr.clone());
    while let Some(request) = packets.incoming().next().await {
        let mut socket = request.accept(None).await?;
        let ep = addr.clone();
        let info = ConnectionInfo {
            endpoint: ep.clone(),
            ip_addr: socket.settings().remote.to_string(),
        };
        let (tx, rx) = unbounded_channel();
        if let Ok(mut pipeline) = builder.build_for(info, rx).await {
            std::thread::spawn(move || loop {
                if let Err(e) = pipeline.run() {
                    error!("Pipeline error: {}\n{}", e, e.backtrace());
                    break;
                }
            });
            tokio::spawn(async move {
                info!("New client connected: {}", ep);
                while let Ok(Some((_inst, bytes))) = socket.try_next().await {
                    if let Err(e) = tx.send(bytes) {
                        warn!("SRT Error: {e}");
                        break;
                    }
                }
                socket.close_and_finish().await.unwrap();
                info!("Client {} disconnected", ep);
            });
        }
    }
    Ok(())
}
