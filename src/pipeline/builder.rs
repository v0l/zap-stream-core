use tokio::sync::mpsc::UnboundedReceiver;
use crate::ingress::ConnectionInfo;
use crate::pipeline::runner::PipelineRunner;
use crate::webhook::Webhook;

#[derive(Clone)]
pub struct PipelineBuilder {
    webhook: Webhook,
}

impl PipelineBuilder {
    pub fn new(webhook: Webhook) -> Self {
        Self { webhook }
    }

    pub async fn build_for(&self, info: ConnectionInfo, recv: UnboundedReceiver<bytes::Bytes>) -> Result<PipelineRunner, anyhow::Error> {
        let config = self.webhook.start(info).await?;
        Ok(PipelineRunner::new(config, recv))
    }
}
