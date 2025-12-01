use anyhow::Result;
use async_trait::async_trait;
use zap_stream_db::{IngestEndpoint, User};

/// Backend abstraction for streaming services
/// Provides data (URLs, viewer counts) without handling event lifecycle
#[async_trait]
pub trait StreamingBackend: Send + Sync {
    /// Get HLS playback URL for a stream
    async fn get_hls_url(&self, stream_id: &str) -> Result<String>;
    
    /// Get recording URL for a stream (if available)
    async fn get_recording_url(&self, stream_id: &str) -> Result<Option<String>>;
    
    /// Get thumbnail URL for a stream
    async fn get_thumbnail_url(&self, stream_id: &str) -> Result<String>;
    
    /// Get current viewer count for a stream
    async fn get_viewer_count(&self, stream_id: &str) -> Result<u32>;
    
    /// Get ingest endpoints for a user
    async fn get_ingest_endpoints(&self, user: &User, endpoints: &[IngestEndpoint]) -> Result<Vec<Endpoint>>;
    
    /// Setup webhooks (for Cloudflare backend, noop for RTMP)
    async fn setup_webhooks(&self, webhook_url: &str) -> Result<()>;
}

/// Endpoint information returned to API clients
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub url: String,
    pub key: String,
    pub capabilities: Vec<String>,
    pub cost: EndpointCost,
}

#[derive(Debug, Clone)]
pub struct EndpointCost {
    pub unit: String,
    pub rate: f32,
}
