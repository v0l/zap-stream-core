use crate::pipeline::runner::PipelineRunner;
use crate::settings::Settings;
use crate::webhook::Webhook;
use anyhow::Result;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::io::Read;

pub mod file;
#[cfg(feature = "srt")]
pub mod srt;
pub mod tcp;
#[cfg(feature = "test-source")]
pub mod test;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Endpoint of the ingress
    pub endpoint: String,

    /// IP address of the connection
    pub ip_addr: String,
}

pub(crate) fn spawn_pipeline(
    info: ConnectionInfo,
    settings: Settings,
    reader: Box<dyn Read + Send>,
) {
    info!("New client connected: {}", &info.ip_addr);
    std::thread::spawn(move || unsafe {
        if let Err(e) = spawn_pipeline_inner(info, settings, reader) {
            error!("{}", e);
        }
    });
}

unsafe fn spawn_pipeline_inner(
    info: ConnectionInfo,
    settings: Settings,
    reader: Box<dyn Read + Send>,
) -> Result<()> {
    let webhook = Webhook::new(settings.clone());
    let mut pl = PipelineRunner::new(info, webhook, reader)?;
    loop {
        pl.run()?
    }
}
