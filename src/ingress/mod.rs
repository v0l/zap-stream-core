use crate::overseer::Overseer;
use crate::pipeline::runner::PipelineRunner;
use crate::settings::Settings;
use anyhow::Result;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::Arc;
use tokio::runtime::Handle;

pub mod file;
#[cfg(feature = "srt")]
pub mod srt;
pub mod tcp;
#[cfg(feature = "test-pattern")]
pub mod test;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Endpoint of the ingress
    pub endpoint: String,

    /// IP address of the connection
    pub ip_addr: String,

    /// Stream key
    pub key: String,
}

pub async fn spawn_pipeline(
    info: ConnectionInfo,
    seer: Arc<dyn Overseer>,
    reader: Box<dyn Read + Send>,
) {
    info!("New client connected: {}", &info.ip_addr);
    let handle = Handle::current();
    let seer = seer.clone();
    std::thread::spawn(move || unsafe {
        match PipelineRunner::new(handle, seer, info, reader) {
            Ok(mut pl) => loop {
                if let Err(e) = pl.run() {
                    error!("Pipeline run failed: {}", e);
                    break;
                }
            },
            Err(e) => {
                error!("Failed to create PipelineRunner: {}", e);
                return;
            }
        };
    });
}
