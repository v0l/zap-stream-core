use std::io;

use log::{error, info, warn};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;

pub async fn listen(addr: String, builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    let listener = TcpListener::bind(addr.clone()).await.unwrap();

    info!("TCP listening on: {}", addr.clone());
    while let Ok((mut socket, ip)) = listener.accept().await {
        info!("New client connected: {}", ip.clone());
        let ep = addr.clone();
        let builder = builder.clone();
        tokio::spawn(async move {
            let (sender, recv) = unbounded_channel();
            let info = ConnectionInfo {
                ip_addr: ip.to_string(),
                endpoint: ep,
            };

            if let Ok(mut pl) = builder.build_for(info, recv).await {
                std::thread::spawn(move || loop {
                    if let Err(e) = pl.run() {
                        error!("Pipeline error: {}\n{}", e, e.backtrace());
                        break;
                    }
                });

                let mut buf = [0u8; 4096];
                loop {
                    match socket.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let bytes = bytes::Bytes::copy_from_slice(&buf[0..n]);
                            if let Err(e) = sender.send(bytes) {
                                warn!("{:?}", e);
                                break;
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                            continue;
                        }
                        Err(e) => {
                            error!("{}", e);
                            break;
                        }
                    }
                }
                info!("Client disconnected: {}", ip);
            }
        });
    }
    Ok(())
}
