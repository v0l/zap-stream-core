use crate::overseer::Overseer;
use anyhow::Result;
use std::sync::Arc;

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
