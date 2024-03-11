use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;
use crate::pipeline::runner::PipelineRunner;
use futures_util::{StreamExt, TryStreamExt};
use log::{info, warn};
use srt_tokio::{SrtListener, SrtSocket};

pub async fn listen_srt(port: u16, pipeline: PipelineBuilder) -> Result<(), anyhow::Error> {
    let (_binding, mut packets) = SrtListener::builder().bind(port).await?;

    while let Some(request) = packets.incoming().next().await {
        let mut socket = request.accept(None).await?;
        let pipeline = pipeline.clone();
        tokio::spawn(async move {
            let info = ConnectionInfo {};
            if let Ok(pl) = pipeline.build_for(info).await {
                let mut stream = SrtStream::new(socket);
                stream.run(pl).await;
            } else {
                socket.close_and_finish().await.unwrap();
            }
        });
    }
    Ok(())
}

struct SrtStream {
    socket: SrtSocket,
    prev: Option<(bytes::Bytes, usize)>,
}

impl SrtStream {
    pub fn new(socket: SrtSocket) -> Self {
        Self { socket, prev: None }
    }

    pub async fn run(&mut self, mut pipeline: PipelineRunner) {
        let socket_id = self.socket.settings().remote_sockid.0;
        let client_desc = format!(
            "(ip_port: {}, socket_id: {}, stream_id: {:?})",
            self.socket.settings().remote,
            socket_id,
            self.socket.settings().stream_id,
        );
        info!("New client connected: {}", client_desc);
        while let Ok(Some((_inst, bytes))) = self.socket.try_next().await {
            if let Err(e) = pipeline.push(bytes).await {
                warn!("{:?}", e);
                break;
            }
        }
        info!("Client {} disconnected", client_desc);
    }
}
