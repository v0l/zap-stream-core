use crate::egress::EgressType;
use crate::variant::VariantStream;
use anyhow::Result;
use ffmpeg_rs_raw::{AvFrameRef, Muxer};
use uuid::Uuid;

/// Trait for services which interact with the decoded input stream
pub trait PipelinePlugin: Send + Sync {
    fn id(&self) -> Uuid;
    fn process_frame(&self, frame: AvFrameRef);
    fn get_frame(&self) -> Option<AvFrameRef>;
    fn configure_egress(&self, e: ConfigurableEgress) -> Result<PipelinePluginConfigurationResult>;
}

pub enum ConfigurableEgress<'a> {
    /// A muxer instance which can be configured with additional stream data by a plugin
    Muxer {
        egress_type: &'a EgressType,
        muxer: &'a mut Muxer,
    },
}

#[derive(Clone, Default)]
pub struct PipelinePluginConfigurationResult {
    /// Variants created as a result of configuring an egress which should be added to egress mappings
    pub variants: Vec<VariantStream>,
}
