use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use crate::settings::Settings;
use anyhow::Result;
use log::info;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn listen(path: PathBuf, overseer: Arc<dyn Overseer>) -> Result<()> {
    info!("Sending file {}", path.to_str().unwrap());

    let info = ConnectionInfo {
        ip_addr: "127.0.0.1:6969".to_string(),
        endpoint: "file-input".to_owned(),
        key: "".to_string(),
    };
    let file = std::fs::File::open(path)?;
    spawn_pipeline(info, overseer.clone(), Box::new(file)).await;

    Ok(())
}
