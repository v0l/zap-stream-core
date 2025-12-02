mod client;
mod types;

pub use client::CloudflareClient;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_db::{IngestEndpoint, User};

use crate::streaming_backend::{Endpoint, EndpointCost, ExternalStreamEvent, StreamingBackend};
use types::CloudflareWebhookPayload;

/// Cloudflare Stream backend implementation
pub struct CloudflareBackend {
    client: CloudflareClient,
    /// Cache mapping stream_id to live_input_uid for HLS URL lookup
    live_input_cache: Arc<RwLock<HashMap<String, String>>>,
    /// Reverse mapping: live_input_uid to stream_id for webhook handling
    reverse_mapping: Arc<RwLock<HashMap<String, String>>>,
    /// Webhook secret for signature verification (stored after setup)
    webhook_secret: Arc<RwLock<Option<String>>>,
}

impl CloudflareBackend {
    /// Create a new Cloudflare backend
    pub fn new(api_token: String, account_id: String) -> Self {
        Self {
            client: CloudflareClient::new(api_token, account_id),
            live_input_cache: Arc::new(RwLock::new(HashMap::new())),
            reverse_mapping: Arc::new(RwLock::new(HashMap::new())),
            webhook_secret: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait]
impl StreamingBackend for CloudflareBackend {
    async fn get_ingest_endpoints(&self, user: &User, db_endpoints: &[IngestEndpoint]) -> Result<Vec<Endpoint>> {
        let mut endpoints = Vec::new();
        
        // For each database endpoint, create a Cloudflare Live Input
        for db_endpoint in db_endpoints {
            let stream_id = user.stream_key.clone();
            let live_input_name = format!("user{}_endpoint{}", user.id, db_endpoint.name);
            
            info!("Creating Cloudflare Live Input: {}", live_input_name);
            
            // Create Live Input via Cloudflare API
            let response = self.client.create_live_input(&live_input_name).await?;
            
            info!("Created Live Input UID: {}", response.result.uid);
            
            // Store mappings for later use
            self.live_input_cache.write().await.insert(
                stream_id.clone(),
                response.result.uid.clone(),
            );
            
            // Store reverse mapping (live_input_uid -> stream_id) for webhook handling
            self.reverse_mapping.write().await.insert(
                response.result.uid.clone(),
                stream_id.clone(),
            );
            
            // Return endpoint with Cloudflare RTMP URL
            endpoints.push(Endpoint {
                name: format!("Cloudflare-{}", db_endpoint.name),
                url: response.result.rtmps.url,
                key: response.result.rtmps.stream_key,
                capabilities: db_endpoint.capabilities
                    .as_ref()
                    .map(|c| c.split(',').map(|s| s.trim().to_string()).collect())
                    .unwrap_or_else(Vec::new),
                cost: EndpointCost {
                    unit: "min".to_string(),
                    rate: db_endpoint.cost as f32 / 1000.0,
                },
            });
        }
        
        Ok(endpoints)
    }

    async fn get_hls_url(&self, stream_id: &str) -> Result<String> {
        // Retrieve live_input_uid from cache
        let cache = self.live_input_cache.read().await;
        let live_input_uid = cache.get(stream_id)
            .ok_or_else(|| anyhow!("Stream '{}' not found in cache", stream_id))?
            .clone();
        drop(cache);
        
        info!("Polling for Video Asset creation for Live Input: {}", live_input_uid);
        
        // Poll Videos API for asset creation (retry up to 30 times = 60 seconds)
        for attempt in 0..30 {
            let response = self.client.get_video_assets(&live_input_uid).await?;
            
            if let Some(asset) = response.result.first() {
                info!("Video Asset found! UID: {}, HLS URL: {}", asset.uid, asset.playback.hls);
                return Ok(asset.playback.hls.clone());
            }
            
            if attempt < 29 {
                if attempt % 5 == 0 {
                    info!("Video Asset not yet created, retrying... (attempt {}/30)", attempt + 1);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        
        Err(anyhow!("Video asset not created after 60 seconds for Live Input {}", live_input_uid))
    }

    async fn get_recording_url(&self, stream_id: &str) -> Result<Option<String>> {
        // Deferred to Step 3D
        warn!("get_recording_url called for stream '{}' but recordings not yet implemented (Step 3D)", stream_id);
        Ok(None)
    }

    async fn get_thumbnail_url(&self, stream_id: &str) -> Result<String> {
        // Deferred to Step 3D - return error for now
        warn!("get_thumbnail_url called for stream '{}' but thumbnails not yet implemented (Step 3D)", stream_id);
        Err(anyhow!("Thumbnails not implemented yet (Step 3D)"))
    }

    async fn get_viewer_count(&self, stream_id: &str) -> Result<u32> {
        // Deferred to Step 3C - return 0 for now
        warn!("get_viewer_count called for stream '{}' but analytics not yet implemented (Step 3C)", stream_id);
        Ok(0)
    }

    async fn setup_webhooks(&self, webhook_url: &str) -> Result<()> {
        info!("Setting up Cloudflare webhook at: {}", webhook_url);
        
        let response = self.client.setup_webhook(webhook_url).await?;
        
        info!("Webhook configured successfully, secret received");
        
        // Store the webhook secret for signature verification
        *self.webhook_secret.write().await = Some(response.result.secret);
        
        Ok(())
    }
    
    fn parse_external_event(&self, payload: &[u8]) -> Result<Option<ExternalStreamEvent>> {
        // Parse webhook payload
        let webhook: CloudflareWebhookPayload = match serde_json::from_slice(payload) {
            Ok(w) => w,
            Err(e) => {
                warn!("Failed to parse Cloudflare webhook payload: {}", e);
                return Ok(None);
            }
        };
        
        info!("Received Cloudflare webhook event: {} for input_id: {}", 
            webhook.data.event_type, webhook.data.input_id);
        
        // Look up stream_id from live_input_uid (blocking is OK here since we're not in async context)
        let stream_id = {
            let reverse_map = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.reverse_mapping.read())
            });
            match reverse_map.get(&webhook.data.input_id) {
                Some(id) => id.clone(),
                None => {
                    warn!("Received webhook for unknown live_input_uid: {}", webhook.data.input_id);
                    return Ok(None);
                }
            }
        };
        
        // Parse stream_id as UUID
        let stream_uuid = match Uuid::parse_str(&stream_id) {
            Ok(u) => u,
            Err(e) => {
                warn!("Invalid stream_id '{}' in reverse mapping: {}", stream_id, e);
                return Ok(None);
            }
        };
        
        // Map Cloudflare event types to our generic events
        match webhook.data.event_type.as_str() {
            "live_input.connected" => {
                info!("Stream connected: {}", stream_uuid);
                
                // Create ConnectionInfo for this stream
                // Note: We use dummy values since Cloudflare doesn't provide this info
                let conn_info = ConnectionInfo {
                    id: stream_uuid,
                    endpoint: "cloudflare", // Cloudflare RTMPS
                    app_name: "cloudflare".to_string(),
                    key: stream_id.clone(),
                    ip_addr: "cloudflare".to_string(), // No IP provided by webhook
                };
                
                Ok(Some(ExternalStreamEvent::Connected {
                    connection_info: conn_info,
                }))
            }
            "live_input.disconnected" => {
                info!("Stream disconnected: {}", stream_uuid);
                Ok(Some(ExternalStreamEvent::Disconnected {
                    stream_id: stream_uuid,
                }))
            }
            "live_input.errored" => {
                warn!("Stream error for {}, treating as disconnection", stream_uuid);
                Ok(Some(ExternalStreamEvent::Disconnected {
                    stream_id: stream_uuid,
                }))
            }
            _ => {
                warn!("Unknown Cloudflare event type: {}", webhook.data.event_type);
                Ok(None)
            }
        }
    }
}
