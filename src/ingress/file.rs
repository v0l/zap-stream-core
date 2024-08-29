use std::path::PathBuf;

use log::{error, info};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::unbounded_channel;

use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;

pub async fn listen(path: PathBuf, builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    info!("Sending file {}", path.to_str().unwrap());

    tokio::spawn(async move {
        let (tx, rx) = unbounded_channel();
        let info = ConnectionInfo {
            ip_addr: "".to_owned(),
            endpoint: "file-input".to_owned(),
        };

        if let Ok(mut pl) = builder.build_for(info, rx).await {
            std::thread::spawn(move || loop {
                if let Err(e) = pl.run() {
                    error!("Pipeline error: {}\n{}", e, e.backtrace());
                    break;
                }
            });

            if let Ok(mut stream) = tokio::fs::File::open(path).await {
                let mut buf = [0u8; 1500];
                loop {
                    if let Ok(r) = stream.read(&mut buf).await {
                        if r > 0 {
                            if let Err(e) = tx.send(bytes::Bytes::copy_from_slice(&buf[..r])) {
                                error!("Failed to send file: {}", e);
                                break;
                            }
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                info!("EOF");
            }
        }
    });
    Ok(())
}
