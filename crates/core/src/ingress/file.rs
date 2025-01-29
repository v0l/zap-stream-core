use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use log::info;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Handle;

pub async fn listen(out_dir: String, path: PathBuf, overseer: Arc<dyn Overseer>) -> Result<()> {
    info!("Sending file: {}", path.display());

    let info = ConnectionInfo {
        ip_addr: "127.0.0.1:6969".to_string(),
        endpoint: "file-input".to_owned(),
        app_name: "".to_string(),
        key: "test".to_string(),
    };
    let file = std::fs::File::open(path)?;
    spawn_pipeline(
        Handle::current(),
        info,
        out_dir.clone(),
        overseer.clone(),
        Box::new(file),
    );

    Ok(())
}
