use anyhow::Result;
use std::sync::Arc;
use zap_stream_core::overseer::Overseer;

/// Monitor stream status, perform any necessary cleanup
pub struct BackgroundMonitor {
    overseer: Arc<dyn Overseer>,
}

impl BackgroundMonitor {
    pub fn new(overseer: Arc<dyn Overseer>) -> Self {
        Self { overseer }
    }

    pub async fn check(&mut self) -> Result<()> {
        self.overseer.check_streams().await
    }
}
