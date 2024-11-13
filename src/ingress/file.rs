use anyhow::Result;
use log::info;
use std::path::PathBuf;

use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::settings::Settings;

pub async fn listen(path: PathBuf, settings: Settings) -> Result<()> {
    info!("Sending file {}", path.to_str().unwrap());

    let info = ConnectionInfo {
        ip_addr: "127.0.0.1:6969".to_string(),
        endpoint: "file-input".to_owned(),
    };
    let file = std::fs::File::open(path)?;
    spawn_pipeline(info, settings, Box::new(file));

    Ok(())
}
