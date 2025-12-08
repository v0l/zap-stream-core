mod client;
mod types;

pub use client::CloudflareClient;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use nostr_sdk::{PublicKey, ToBech32};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;
use zap_stream_db::{IngestEndpoint, User};

use crate::streaming_backend::{Endpoint, EndpointCost, ExternalStreamEvent, StreamingBackend};
use types::CloudflareWebhookPayload;

/// Stream information stored in cache
#[derive(Clone, Debug)]
struct StreamInfo {
    live_input_uid: String,
    hls_url: Option<String>,
}

/// Cloudflare Stream backend implementation
pub struct CloudflareBackend {
    client: CloudflareClient,
    /// Cache mapping stream_id to stream info (live_input_uid + HLS URL)
    live_input_cache: Arc<RwLock<HashMap<String, StreamInfo>>>,
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
    async fn generate_stream_key(&self, pubkey: &[u8; 32]) -> Result<String> {
        let pk = PublicKey::from_slice(pubkey)?;
        let live_input_name = pk.to_bech32()?;
        info!("Creating Cloudflare Live Input for new user: {}", live_input_name);
        
        let response = self.client.create_live_input(&live_input_name).await?;
        let live_input_uid = response.result.uid.clone();
        
        info!("Created Cloudflare Live Input UID: {}", live_input_uid);
        
        // Store the mapping for later use (HLS URL will be populated when first requested)
        self.live_input_cache.write().await.insert(
            live_input_uid.clone(),
            StreamInfo {
                live_input_uid: live_input_uid.clone(),
                hls_url: None,
            },
        );
        
        Ok(live_input_uid)
    }
    
    fn is_valid_stream_key(&self, key: &str) -> bool {
        // Cloudflare Live Input UIDs are 32 lowercase hexadecimal characters
        key.len() == 32 && key.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    }
    
    async fn get_ingest_endpoints(&self, user: &User, db_endpoints: &[IngestEndpoint]) -> Result<Vec<Endpoint>> {
        let mut endpoints = Vec::new();
        
        // Use the persistent stream_key (which IS the Cloudflare Live Input UID)
        let live_input_uid = user.stream_key.clone();
        
        // Fetch current RTMPS details from Cloudflare (source of truth)
        // If the Live Input doesn't exist, the UUID is invalid/stale
        let live_input = match self.client.get_live_input(&live_input_uid).await {
            Ok(input) => input,
            Err(e) => {
                warn!("Failed to fetch Live Input '{}': {}. User may need to regenerate UUID.", live_input_uid, e);
                bail!("UUID is invalid or expired.");
            }
        };
        
        // Store base URL and stream key separately (consistent with RML RTMP backend)
        let rtmps_base_url = live_input.result.rtmps.url.clone();
        let rtmps_stream_key = live_input.result.rtmps.stream_key.clone();
        
        // Store mapping for later HLS lookup (HLS URL will be populated when first requested)
        self.live_input_cache.write().await.insert(
            live_input_uid.clone(),
            StreamInfo {
                live_input_uid: live_input_uid.clone(),
                hls_url: None,
            },
        );
        
        // For each database endpoint tier, return base URL and key separately
        // (matches RML RTMP backend pattern for DX consistency)
        for db_endpoint in db_endpoints {
            endpoints.push(Endpoint {
                name: format!("Cloudflare-{}", db_endpoint.name),
                url: rtmps_base_url.clone(),
                key: rtmps_stream_key.clone(),
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
        // Check if HLS URL is already cached
        {
            let cache = self.live_input_cache.read().await;
            if let Some(info) = cache.get(stream_id) {
                if let Some(hls_url) = &info.hls_url {
                    info!("Using cached HLS URL for stream {}", stream_id);
                    return Ok(hls_url.clone());
                }
            }
        }
        
        // Retrieve live_input_uid from cache
        let live_input_uid = {
            let cache = self.live_input_cache.read().await;
            cache.get(stream_id)
                .ok_or_else(|| anyhow!("Stream '{}' not found in cache", stream_id))?
                .live_input_uid
                .clone()
        };
        
        info!("Polling for Video Asset creation for Live Input: {}", live_input_uid);
        
        // Poll Videos API for asset creation (retry up to 30 times = 60 seconds)
        for attempt in 0..30 {
            let response = self.client.get_video_assets(&live_input_uid).await?;
            
            if let Some(asset) = response.result.first() {
                let hls_url = asset.playback.hls.clone();
                info!("Video Asset found! UID: {}, HLS URL: {}", asset.uid, hls_url);
                
                // Cache the HLS URL for future use
                {
                    let mut cache = self.live_input_cache.write().await;
                    if let Some(info) = cache.get_mut(stream_id) {
                        info.hls_url = Some(hls_url.clone());
                    }
                }
                
                return Ok(hls_url);
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
        // Get cached HLS URL
        let hls_url = {
            let cache = self.live_input_cache.read().await;
            cache.get(stream_id).and_then(|info| info.hls_url.clone())
        };
        
        let hls_url = match hls_url {
            Some(url) => url,
            None => {
                // Stream not live yet or HLS URL not cached
                info!("No HLS URL cached for stream {}, returning 0 viewers", stream_id);
                return Ok(0);
            }
        };
        
        // Transform HLS URL to viewer count URL
        // FROM: https://customer-{CODE}.cloudflarestream.com/{UID}/manifest/video.m3u8
        // TO:   https://customer-{CODE}.cloudflarestream.com/{UID}/views
        let viewer_url = hls_url.replace("/manifest/video.m3u8", "/views");
        
        // Fetch viewer count (no authentication needed for this endpoint)
        let response = match reqwest::get(&viewer_url).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("Failed to fetch viewer count from Cloudflare: {}", e);
                return Ok(0); // Fallback to 0 on network error
            }
        };
        
        let json: serde_json::Value = match response.json().await {
            Ok(j) => j,
            Err(e) => {
                warn!("Failed to parse viewer count JSON: {}", e);
                return Ok(0); // Fallback to 0 on parse error
            }
        };
        
        let count = json["liveViewers"].as_u64().unwrap_or(0) as u32;
        info!("Cloudflare viewer count for stream {}: {}", stream_id, count);
        Ok(count)
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
        // Log raw webhook payload for debugging
        let payload_str = String::from_utf8_lossy(payload);
        info!("Raw Cloudflare webhook payload: {}", payload_str);
        
        // Parse webhook payload
        let webhook: CloudflareWebhookPayload = match serde_json::from_slice(payload) {
            Ok(w) => w,
            Err(e) => {
                warn!("Failed to parse Cloudflare webhook payload: {}", e);
                warn!("Payload was: {}", payload_str);
                return Ok(None);
            }
        };
        
        info!("Received Cloudflare webhook event: {} for input_id: {}", 
            webhook.data.event_type, webhook.data.input_id);
        
        // Map Cloudflare event types to our generic events
        match webhook.data.event_type.as_str() {
            "live_input.connected" => {
                info!("Stream connected for input_uid: {}", webhook.data.input_id);
                Ok(Some(ExternalStreamEvent::Connected {
                    input_uid: webhook.data.input_id,
                    // Cloudflare webhooks don't include tier info
                    // Default to "Basic" (free tier) for now
                    // TODO: Future enhancement - look up user's preferred tier from DB
                    app_name: "Basic".to_string(),
                }))
            }
            "live_input.disconnected" | "live_input.errored" => {
                info!("Stream disconnected for input_uid: {}", webhook.data.input_id);
                Ok(Some(ExternalStreamEvent::Disconnected {
                    input_uid: webhook.data.input_id,
                }))
            }
            _ => {
                warn!("Unknown Cloudflare event type: {}", webhook.data.event_type);
                Ok(None)
            }
        }
    }
    
    fn register_stream_mapping(&self, input_uid: &str, stream_id: Uuid) -> Result<()> {
        // Populate reverse_mapping: input_uid -> stream_id (for disconnect webhook)
        let mut reverse = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.reverse_mapping.write())
        });
        reverse.insert(input_uid.to_string(), stream_id.to_string());
        drop(reverse);
        
        // Populate live_input_cache: stream_id -> StreamInfo (for HLS URL lookup)
        let mut cache = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.live_input_cache.write())
        });
        cache.insert(
            stream_id.to_string(),
            StreamInfo {
                live_input_uid: input_uid.to_string(),
                hls_url: None,
            },
        );
        drop(cache);
        
        info!("Registered mapping: input_uid {} <-> stream_id {}", input_uid, stream_id);
        Ok(())
    }
    
    fn get_stream_id_for_input_uid(&self, input_uid: &str) -> Result<Option<Uuid>> {
        let mapping = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.reverse_mapping.read())
        });
        
        match mapping.get(input_uid) {
            Some(stream_id_str) => {
                match Uuid::parse_str(stream_id_str) {
                    Ok(uuid) => Ok(Some(uuid)),
                    Err(e) => {
                        warn!("Invalid UUID in mapping for input_uid {}: {}", input_uid, e);
                        Ok(None)
                    }
                }
            }
            None => Ok(None),
        }
    }
    
    fn remove_stream_mapping(&self, input_uid: &str) -> Result<()> {
        let mut mapping = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.reverse_mapping.write())
        });
        mapping.remove(input_uid);
        info!("Removed mapping for input_uid {}", input_uid);
        Ok(())
    }
}
