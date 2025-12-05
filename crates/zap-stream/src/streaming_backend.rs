use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_db::{IngestEndpoint, User};

/// Backend abstraction for streaming services
/// Provides data (URLs, viewer counts) without handling event lifecycle
#[async_trait]
pub trait StreamingBackend: Send + Sync {
    /// Generate a backend-specific stream key for a user
    async fn generate_stream_key(&self, pubkey: &[u8; 32]) -> Result<String>;
    
    /// Check if a stream key is valid for this backend
    fn is_valid_stream_key(&self, key: &str) -> bool;
    
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
    
    /// Setup webhooks for backends that support external event notifications
    /// Backends using listeners (like RML RTMP) can implement this as a no-op
    async fn setup_webhooks(&self, webhook_url: &str) -> Result<()>;
    
    /// Parse backend-specific external event (webhook) into generic stream event
    /// Returns None if the payload is not for this backend or cannot be parsed
    fn parse_external_event(&self, payload: &[u8]) -> Result<Option<ExternalStreamEvent>>;
}

/// External stream events from backend providers (webhooks, etc.)
#[derive(Debug, Clone)]
pub enum ExternalStreamEvent {
    /// Stream connection started
    Connected {
        connection_info: ConnectionInfo,
    },
    /// Stream connection ended
    Disconnected {
        stream_id: Uuid,
    },
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
