use crate::demux::Demuxer;
use crate::ingress::ConnectionInfo;
use crate::pipeline::runner::PipelineRunner;
use crate::pipeline::PipelineStep;
use crate::webhook::Webhook;

#[derive(Clone)]
pub struct PipelineBuilder {
    webhook: Webhook,
}

impl PipelineBuilder {
    pub fn new(webhook: Webhook) -> Self {
        Self { webhook }
    }

    pub async fn build_for(&self, info: ConnectionInfo) -> Result<PipelineRunner, anyhow::Error> {
        let config = self.webhook.start(info).await?;

        let mut steps: Vec<Box<dyn PipelineStep + Sync + Send>> = Vec::new();
        steps.push(Box::new(Demuxer::new()));

        Ok(PipelineRunner::new(steps))
    }
}
