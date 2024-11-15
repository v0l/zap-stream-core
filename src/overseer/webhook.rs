use crate::ingress::ConnectionInfo;
use crate::overseer::{IngressInfo, Overseer};
use crate::pipeline::PipelineConfig;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone)]
pub struct WebhookOverseer {
    url: String,
}

impl WebhookOverseer {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
        }
    }
}

#[async_trait]
impl Overseer for WebhookOverseer {
    async fn start_stream(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig> {
        todo!()
    }

    async fn on_segment(
        &self,
        pipeline_id: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()> {
        todo!()
    }
}
