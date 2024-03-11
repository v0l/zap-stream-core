use crate::ingress::ConnectionInfo;

#[derive(Clone)]
pub(crate) struct Webhook {
    url: String,
}

impl Webhook {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub async fn start(&self, connection_info: ConnectionInfo) -> Result<PipelineConfig, anyhow::Error> {
        Ok(PipelineConfig {})
    }
}

pub(crate) struct PipelineConfig {}
