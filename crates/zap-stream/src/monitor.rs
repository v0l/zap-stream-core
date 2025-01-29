use crate::overseer::ZapStreamOverseer;
use anyhow::Result;
use std::sync::Arc;
use zap_stream_core::overseer::Overseer;

/// Monitor stream status, perform any necessary cleanup
pub struct BackgroundMonitor {
    overseer: Arc<ZapStreamOverseer>,
}

impl BackgroundMonitor {
    pub fn new(overseer: Arc<ZapStreamOverseer>) -> Self {
        Self { overseer }
    }

    pub async fn check(&mut self) -> Result<()> {
        self.overseer.check_streams().await
    }
}
