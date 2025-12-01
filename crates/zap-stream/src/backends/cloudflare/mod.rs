mod client;
mod types;

pub use client::CloudflareClient;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};
use zap_stream_db::{IngestEndpoint, User};

use crate::streaming_backend::{Endpoint, EndpointCost, StreamingBackend};

/// Cloudflare Stream backend implementation
pub struct CloudflareBackend {
    client: CloudflareClient,
    /// Cache mapping stream_id to live_input_uid for HLS URL lookup
    live_input_cache: Arc<RwLock<HashMap<String, String>>>,
}

impl CloudflareBackend {
    /// Create a new Cloudflare backend
    pub fn new(api_token: String, account_id: String) -> Self {
        Self {
            client: CloudflareClient::new(api_token, account_id),
            live_input_cache: Arc::new(RwLock::new(HashMap::new())),
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
            
            // Store mapping for later use (stream_id -> live_input_uid)
            self.live_input_cache.write().await.insert(
                stream_id.clone(),
                response.result.uid.clone(),
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
        // Deferred to Step 3B - no-op for now
        warn!("setup_webhooks called with URL '{}' but webhooks not yet implemented (Step 3B)", webhook_url);
        Ok(())
    }
}
