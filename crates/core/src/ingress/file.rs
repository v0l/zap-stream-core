use crate::ingress::{ConnectionInfo, spawn_pipeline};
use crate::overseer::Overseer;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::{error, info};
use uuid::Uuid;

pub async fn listen(out_dir: String, path: PathBuf, overseer: Arc<dyn Overseer>) -> Result<()> {
    info!("Sending file: {}", path.display());

    let info = ConnectionInfo {
        id: Uuid::new_v4(),
        ip_addr: "127.0.0.1:6969".to_string(),
        endpoint: "file-input".to_string(),
        app_name: "".to_string(),
        key: "test".to_string(),
    };
    let out_dir = PathBuf::from(out_dir).join(info.id.to_string());
    if !out_dir.exists() {
        std::fs::create_dir_all(&out_dir)?;
    }
    let url = path.to_str().unwrap().to_string();
    let file = std::fs::File::open(path)?;
    if let Err(e) = spawn_pipeline(
        Handle::current(),
        info,
        out_dir.clone(),
        overseer.clone(),
        Box::new(file),
        Some(url),
        None,
    ) {
        error!("Failed to spawn pipeline: {}", e);
    }

    Ok(())
}
