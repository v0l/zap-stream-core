use crate::egress::EgressConfig;
use crate::ingress::ConnectionInfo;
use crate::overseer::{get_default_variants, IngressInfo, Overseer};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::variant::StreamMapping;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

/// Simple static file output without any access controls
/// Useful for testing or self-hosting
pub struct LocalOverseer;

impl LocalOverseer {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Overseer for LocalOverseer {
    async fn check_streams(&self) -> Result<()> {
        todo!()
    }

    async fn start_stream(
        &self,
        _connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig> {
        let vars = get_default_variants(stream_info)?;
        let var_ids = vars.iter().map(|v| v.id()).collect();
        Ok(PipelineConfig {
            id: Uuid::new_v4(),
            variants: vars,
            egress: vec![EgressType::HLS(EgressConfig {
                name: "HLS".to_owned(),
                variants: var_ids,
            })],
        })
    }

    async fn on_segment(
        &self,
        pipeline_id: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()> {
        // nothing to do here
        Ok(())
    }

    async fn on_thumbnail(
        &self,
        pipeline_id: &Uuid,
        width: usize,
        height: usize,
        path: &PathBuf,
    ) -> Result<()> {
        // nothing to do here
        Ok(())
    }

    async fn on_end(&self, pipeline_id: &Uuid) -> Result<()> {
        // nothing to do here
        Ok(())
    }
}
