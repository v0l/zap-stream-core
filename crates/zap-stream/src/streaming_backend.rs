use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;
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
    
    /// Check if viewer count has changed since last check
    /// Returns true if count changed or enough time has passed for a periodic update
    /// This enables real-time viewer count updates in Nostr events
    async fn check_and_update_viewer_count(&self, stream_id: &str) -> Result<bool>;
    
    /// Check if stream is healthy and active
    /// Returns (is_active, should_timeout)
    /// - is_active: Whether stream has recent activity
    /// - should_timeout: Whether stream should be ended due to timeout
    async fn check_stream_status(&self, stream_id: &str) -> (bool, bool);
    
    /// Get ingest endpoints for a user
    async fn get_ingest_endpoints(&self, user: &User, endpoints: &[IngestEndpoint]) -> Result<Vec<Endpoint>>;
    
    /// Setup webhooks for backends that support external event notifications
    /// Backends using listeners (like RML RTMP) can implement this as a no-op
    async fn setup_webhooks(&self, webhook_url: &str) -> Result<()>;
    
    /// Parse backend-specific external event (webhook) into generic stream event
    /// Returns None if the payload is not for this backend or cannot be parsed
    fn parse_external_event(&self, payload: &[u8]) -> Result<Option<ExternalStreamEvent>>;
    
    /// Register a mapping from input_uid to stream_id
    /// Used by webhook-based backends to track active streams
    /// For Cloudflare: input_uid is the Live Input UID
    fn register_stream_mapping(&self, input_uid: &str, stream_id: Uuid) -> Result<()>;
    
    /// Look up stream_id from input_uid
    /// For Cloudflare: input_uid is the Live Input UID
    /// Returns None if no mapping exists
    fn get_stream_id_for_input_uid(&self, input_uid: &str) -> Result<Option<Uuid>>;
    
    /// Remove stream mapping when stream ends
    /// For Cloudflare: input_uid is the Live Input UID
    fn remove_stream_mapping(&self, input_uid: &str) -> Result<()>;
}

/// External stream events from backend providers (webhooks, etc.)
#[derive(Debug, Clone)]
pub enum ExternalStreamEvent {
    /// Stream connection started
    Connected {
        /// Identifier to look up user (e.g., Cloudflare Live Input UID)
        /// This matches what's stored in DB as user.stream_key
        input_uid: String,
        /// App name for endpoint detection (e.g., "Basic", "Good")
        /// For Cloudflare: defaults to "Basic" since webhook has no tier info
        /// TODO: Future enhancement - store user's preferred tier in DB
        app_name: String,
    },
    /// Stream connection ended
    Disconnected {
        /// Identifier to look up the stream (e.g., Cloudflare Live Input UID)
        input_uid: String,
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
