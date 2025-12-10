mod client;
mod types;

pub use client::CloudflareClient;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nostr_sdk::{PublicKey, ToBech32};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;
use zap_stream_db::{IngestEndpoint, User};

use crate::streaming_backend::{Endpoint, EndpointCost, ExternalStreamEvent, StreamingBackend};
use types::{LiveInputWebhook, VideoAssetWebhook};

/// Stream information stored in cache
#[derive(Clone, Debug)]
struct StreamInfo {
    live_input_uid: String,
    hls_url: Option<String>,
    recording_url: Option<String>,
    thumbnail_url: Option<String>,
}

/// Viewer count cache entry with timestamp
#[derive(Clone, Debug)]
struct ViewerCountCache {
    count: u32,
    timestamp: Instant,
}

/// Viewer count state for change detection
#[derive(Clone, Debug)]
struct ViewerCountState {
    last_published_count: u32,
    last_update_time: DateTime<Utc>,
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
    /// Viewer count cache with 30-second TTL to prevent API spam
    viewer_count_cache: Arc<RwLock<HashMap<String, ViewerCountCache>>>,
    /// Track viewer count states for change detection
    viewer_count_states: Arc<RwLock<HashMap<String, ViewerCountState>>>,
    /// Minimum update interval in minutes (matches RML RTMP behavior)
    min_update_minutes: i64,
    /// Cache duration for viewer counts (30 seconds)
    cache_duration: Duration,
    /// Custom ingest domain (if configured)
    custom_ingest_domain: Option<String>,
}

impl CloudflareBackend {
    /// Create a new Cloudflare backend
    pub fn new(api_token: String, account_id: String, endpoints_public_hostname: String) -> Self {
        // Use custom ingest domain if configured (not empty and not localhost)
        let custom_ingest_domain = if !endpoints_public_hostname.is_empty() 
            && endpoints_public_hostname != "localhost" {
            Some(endpoints_public_hostname)
        } else {
            None
        };
        
        Self {
            client: CloudflareClient::new(api_token, account_id),
            live_input_cache: Arc::new(RwLock::new(HashMap::new())),
            reverse_mapping: Arc::new(RwLock::new(HashMap::new())),
            webhook_secret: Arc::new(RwLock::new(None)),
            viewer_count_cache: Arc::new(RwLock::new(HashMap::new())),
            viewer_count_states: Arc::new(RwLock::new(HashMap::new())),
            min_update_minutes: 10,
            cache_duration: Duration::from_secs(30),
            custom_ingest_domain,
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
                recording_url: None,
                thumbnail_url: None,
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
        let mut rtmps_base_url = live_input.result.rtmps.url.clone();
        let rtmps_stream_key = live_input.result.rtmps.stream_key.clone();
        
        // If custom ingest domain is configured, replace Cloudflare hostname with custom domain
        if let Some(custom_domain) = &self.custom_ingest_domain {
            if !custom_domain.is_empty() && custom_domain != "localhost" {
                // Parse the Cloudflare URL and replace hostname
                // FROM: rtmps://live.cloudflare.com:443/live/
                // TO:   rtmps://custom.domain.com:443/live/
                if let Ok(mut url) = url::Url::parse(&rtmps_base_url) {
                    if url.set_host(Some(custom_domain)).is_ok() {
                        rtmps_base_url = url.to_string();
                        info!("Using custom ingest domain: {}", rtmps_base_url);
                    }
                }
            }
        }
        
        // Store mapping for later HLS lookup (HLS URL will be populated when first requested)
        self.live_input_cache.write().await.insert(
            live_input_uid.clone(),
            StreamInfo {
                live_input_uid: live_input_uid.clone(),
                hls_url: None,
                recording_url: None,
                thumbnail_url: None,
            },
        );
        
        // For each database endpoint tier, return base URL and key separately
        // (matches RML RTMP backend pattern for DX consistency)
        for db_endpoint in db_endpoints {
            endpoints.push(Endpoint {
                name: db_endpoint.name.clone(),
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
        let cache = self.live_input_cache.read().await;
        Ok(cache.get(stream_id).and_then(|info| info.recording_url.clone()))
    }

    async fn get_thumbnail_url(&self, stream_id: &str) -> Result<String> {
        let cache = self.live_input_cache.read().await;
        match cache.get(stream_id).and_then(|info| info.thumbnail_url.clone()) {
            Some(url) => Ok(url),
            None => Err(anyhow!("Thumbnail not yet available for stream {}", stream_id)),
        }
    }

    async fn get_viewer_count(&self, stream_id: &str) -> Result<u32> {
        // Check cache first (30-second TTL)
        {
            let cache = self.viewer_count_cache.read().await;
            if let Some(cached) = cache.get(stream_id) {
                if cached.timestamp.elapsed() < self.cache_duration {
                    return Ok(cached.count);
                }
            }
        }
        
        // Cache miss or expired - fetch from API
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
        info!("Cloudflare API call: viewer count for stream {}: {}", stream_id, count);
        
        // Update cache
        {
            let mut cache = self.viewer_count_cache.write().await;
            cache.insert(
                stream_id.to_string(),
                ViewerCountCache {
                    count,
                    timestamp: Instant::now(),
                },
            );
        }
        
        Ok(count)
    }
    
    async fn check_and_update_viewer_count(&self, stream_id: &str) -> Result<bool> {
        // Fetch current viewer count from Cloudflare
        let viewer_count = self.get_viewer_count(stream_id).await?;
        let now = Utc::now();
        
        let should_update = {
            let viewer_states = self.viewer_count_states.read().await;
            if let Some(state) = viewer_states.get(stream_id) {
                // Update if count changed OR if 10 minutes have passed since last update
                viewer_count != state.last_published_count
                    || (now - state.last_update_time).num_minutes() >= self.min_update_minutes
            } else {
                // First time tracking this stream, always update if viewer count > 0
                viewer_count > 0
            }
        };
        
        if should_update && viewer_count > 0 {
            // Update the tracking state
            let mut viewer_states = self.viewer_count_states.write().await;
            viewer_states.insert(
                stream_id.to_string(),
                ViewerCountState {
                    last_published_count: viewer_count,
                    last_update_time: now,
                },
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    async fn check_stream_status(&self, _stream_id: &str) -> (bool, bool) {
        // Cloudflare streams are managed via webhooks (connected/disconnected events)
        // Return always active, never timeout since lifecycle is webhook-driven
        // TODO: Future enhancement - track active state via webhook events
        (true, false)
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
        let payload_str = String::from_utf8_lossy(payload);
        // Do you need to debug? Here's the payload:
        // info!("Raw Cloudflare webhook payload: {}", payload_str);
        
        // Try parsing a webhook connection test message
        if payload_str.contains("\"text\"") && payload_str.contains("Hello World") {
            info!("Received webhook test message - webhook configuration successful!");
            return Ok(None);
        }
        
        // Try parsing as Live Input webhook first (has "name" field)
        if let Ok(webhook) = serde_json::from_slice::<LiveInputWebhook>(payload) {
            info!("Received Cloudflare webhook event: {} for input_id: {}", 
                webhook.data.event_type, webhook.data.input_id);
            
            // Map Cloudflare event types to our generic events
            return match webhook.data.event_type.as_str() {
                "live_input.connected" => {
                    Ok(Some(ExternalStreamEvent::Connected {
                        input_uid: webhook.data.input_id,
                        // Cloudflare ingest endpoints don't use multiple app_names and so don't include tier info
                        // Leaving app_name here empty for now
                        // Given empty the overseer charges most expensive by default
                        // Good practice for now: Use zap.stream admin to configure ONE endpoint ONLY
                        // TODO: Future enhancement - support multiple endpoints
                        app_name: String::new(),
                    }))
                }
                "live_input.disconnected" | "live_input.errored" => {
                    Ok(Some(ExternalStreamEvent::Disconnected {
                        input_uid: webhook.data.input_id,
                    }))
                }
                _ => {
                    warn!("Unknown Cloudflare event type: {}", webhook.data.event_type);
                    Ok(None)
                }
            };
        }
        
        // Try parsing as Video Asset webhook (no "name" field, has "uid" field)
        if let Ok(video_asset) = serde_json::from_slice::<VideoAssetWebhook>(payload) {
            // Only process if the video is ready
            if video_asset.status.state == "ready" {
                info!("Cloudflare Video Asset ready for input_uid {}, recording: {} thumbnail: {}", 
                    video_asset.live_input, video_asset.playback.hls, video_asset.thumbnail);
                let input_uid = video_asset.live_input.clone();
                let recording_url = video_asset.playback.hls.clone();
                let thumbnail_url = video_asset.thumbnail.clone();
                
                // Look up stream_id from input_uid and update cache
                // Use block_in_place since parse_external_event is not async
                let stream_id_opt = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let mapping = self.reverse_mapping.read().await;
                        mapping.get(&input_uid).cloned()
                    })
                });
                
                if let Some(stream_id) = stream_id_opt {
                    // Update cache with recording info
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            let mut cache = self.live_input_cache.write().await;
                            if let Some(info) = cache.get_mut(&stream_id) {
                                info.recording_url = Some(recording_url.clone());
                                info.thumbnail_url = Some(thumbnail_url.clone());
                                info!("Cached recording URLs for stream {} during webhook parse", stream_id);
                            }
                        })
                    });
                }
                
                return Ok(Some(ExternalStreamEvent::VideoAssetReady {
                    input_uid,
                    recording_url,
                    thumbnail_url,
                    duration: video_asset.duration,
                }));
            } else {
                info!("Video Asset not ready yet (state: {}), ignoring", video_asset.status.state);
                return Ok(None);
            }
        }
        
        // Failed to parse as either type
        warn!("Failed to parse Cloudflare webhook payload as either Live Input or Video Asset");
        warn!("Payload was: {}", payload_str);
        Ok(None)
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
                recording_url: None,
                thumbnail_url: None,
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
        // Delay removal by 60 seconds to catch late-arriving Video Asset webhooks
        // Video Asset webhooks typically arrive 10-30 seconds after disconnect
        let mapping = self.reverse_mapping.clone();
        let input_uid_owned = input_uid.to_string();
        
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let mut m = mapping.write().await;
            m.remove(&input_uid_owned);
            info!("Removed mapping for input_uid {} after 60s", input_uid_owned);
        });
        
        // info!("Scheduled mapping removal for input_uid {} in 60 seconds", input_uid);
        Ok(())
    }
}
