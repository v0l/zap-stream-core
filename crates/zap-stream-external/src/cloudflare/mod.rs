use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;
use zap_stream_api_common::{Endpoint, EndpointCost};
use zap_stream_db::{IngestEndpoint, User};

mod api;
mod client;
mod types;
pub use client::*;
pub use types::*;
pub use api::*;

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
    async fn get_ingest_endpoints(
        &self,
        user: &User,
        db_endpoints: &[IngestEndpoint],
    ) -> Result<Vec<Endpoint>> {
        let mut endpoints = Vec::new();

        // Use the persistent stream_key (which IS the Cloudflare Live Input UID)
        let live_input_uid = user.stream_key.clone();

        // Fetch current RTMPS details from Cloudflare (source of truth)
        // If the Live Input doesn't exist, the UUID is invalid/stale
        let live_input = match self.client.get_live_input(&live_input_uid).await {
            Ok(input) => input,
            Err(e) => {
                warn!(
                    "Failed to fetch Live Input '{}': {}. User may need to regenerate UUID.",
                    live_input_uid, e
                );
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
                if let Ok(mut url) = Url::parse(&rtmps_base_url) {
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
                capabilities: db_endpoint
                    .capabilities
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
            cache
                .get(stream_id)
                .ok_or_else(|| anyhow!("Stream '{}' not found in cache", stream_id))?
                .live_input_uid
                .clone()
        };

        info!(
            "Polling for Video Asset creation for Live Input: {}",
            live_input_uid
        );

        // Poll Videos API for asset creation (retry up to 30 times = 60 seconds)
        for attempt in 0..30 {
            let response = self.client.get_video_assets(&live_input_uid).await?;

            if let Some(asset) = response.result.first() {
                let hls_url = asset.playback.hls.clone();
                info!(
                    "Video Asset found! UID: {}, HLS URL: {}",
                    asset.uid, hls_url
                );

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
                    info!(
                        "Video Asset not yet created, retrying... (attempt {}/30)",
                        attempt + 1
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        Err(anyhow!(
            "Video asset not created after 60 seconds for Live Input {}",
            live_input_uid
        ))
    }

    async fn get_recording_url(&self, stream_id: &str) -> Result<Option<String>> {
        let cache = self.live_input_cache.read().await;
        Ok(cache
            .get(stream_id)
            .and_then(|info| info.recording_url.clone()))
    }

    async fn get_thumbnail_url(&self, stream_id: &str) -> Result<String> {
        let cache = self.live_input_cache.read().await;
        match cache
            .get(stream_id)
            .and_then(|info| info.thumbnail_url.clone())
        {
            Some(url) => Ok(url),
            None => Err(anyhow!(
                "Thumbnail not yet available for stream {}",
                stream_id
            )),
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
                info!(
                    "No HLS URL cached for stream {}, returning 0 viewers",
                    stream_id
                );
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
        info!(
            "Cloudflare API call: viewer count for stream {}: {}",
            stream_id, count
        );

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

}
