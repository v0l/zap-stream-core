use crate::cloudflare::{
    CloudflareClient, CloudflareToken, LiveInput, LiveInputWebhookData, VideoAssetWebhook,
    WebhookPayload, WebhookResult,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{any};
use axum::{Router};
use chrono::Utc;
use nostr_sdk::{Client, Event, JsonUtil, Kind, PublicKey, Tag, ToBech32};
use nostr_sdk::prelude::Coordinate;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::log::error;
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;
use zap_stream::api_base::ApiBase;
use zap_stream::nostr::N53Publisher;
use zap_stream::payments::LightningNode;
use zap_stream::stream_manager::StreamManager;
use zap_stream_api_common::*;
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_db::{IngestEndpoint, StreamKeyType, User, UserStream, UserStreamState, ZapStreamDb};

/// Sentinel error: no live streams found for a user during stream-end processing.
/// Used for downcast-based matching instead of string comparison.
#[derive(Debug)]
struct NoLiveStreams {
    user_id: u64,
    stream_key_id: Option<u64>,
}

impl std::fmt::Display for NoLiveStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "No live streams found for user {} (stream_key_id: {:?})",
            self.user_id, self.stream_key_id
        )
    }
}

impl std::error::Error for NoLiveStreams {}

fn select_ingest_endpoint<'a>(
    endpoints: &'a [IngestEndpoint],
    ingest_id: Option<u64>,
    app_name: &str,
) -> Result<&'a IngestEndpoint> {
    if endpoints.is_empty() {
        bail!("No endpoints found, please configure endpoints first!");
    }
    if let Some(id) = ingest_id {
        let Some(endpoint) = endpoints.iter().find(|endpoint| endpoint.id == id) else {
            bail!("Ingest endpoint not found for user");
        };
        return Ok(endpoint);
    }
    if let Some(selected) = endpoints
        .iter()
        .find(|endpoint| endpoint.name.eq_ignore_ascii_case(app_name))
    {
        return Ok(selected);
    }
    let default = endpoints.iter().min_by_key(|endpoint| endpoint.cost).unwrap();
    Ok(default)
}

fn apply_custom_ingest_domain(base_url: &str, custom_domain: Option<&str>) -> String {
    let Some(domain) = custom_domain else {
        return base_url.to_string();
    };
    if domain.is_empty() || domain == "localhost" {
        return base_url.to_string();
    }
    if let Ok(mut url) = Url::parse(base_url) {
        if url.set_host(Some(domain)).is_ok() {
            return url.to_string();
        }
    }
    base_url.to_string()
}

fn build_account_endpoints(
    input: &LiveInput,
    ingest: &IngestEndpoint,
    custom_domain: Option<&str>,
) -> Vec<Endpoint> {
    let cost = EndpointCost {
        unit: "min".to_string(),
        rate: ingest.cost as f32 / 1000.0,
    };

    let mut endpoints = vec![Endpoint {
        name: format!("RTMPS-{}", ingest.name),
        url: apply_custom_ingest_domain(&input.rtmps.url, custom_domain),
        key: input.rtmps.stream_key.clone(),
        capabilities: vec![],
        cost,
    }];

    // TODO: Re-enable SRT endpoint when app-side SRT support is ready
    // if let Some(srt) = &input.srt {
    //     endpoints.push(Endpoint {
    //         name: format!("SRT-{}", ingest.name),
    //         url: apply_custom_ingest_domain(&srt.url, custom_domain),
    //         key: format!("streamid={}&passphrase={}", srt.stream_id, srt.passphrase),
    //         capabilities: vec![],
    //         cost: EndpointCost {
    //             unit: "min".to_string(),
    //             rate: ingest.cost as f32 / 1000.0,
    //         },
    //     });
    // }

    endpoints
}

fn build_stream_key(row: &zap_stream_db::UserStreamKey, key: String) -> StreamKey {
    StreamKey {
        id: row.id,
        key,
        created: row.created.timestamp(),
        expires: row.expires.map(|e| e.timestamp()),
        stream_id: row.stream_id.clone(),
    }
}

fn apply_video_asset_to_stream(stream: &mut UserStream, asset: &VideoAssetWebhook) -> bool {
    let mut changed = false;
    if stream.external_video_id.as_deref() != Some(asset.uid.as_str()) {
        stream.external_video_id = Some(asset.uid.clone());
        changed = true;
    }
    if stream.thumb.as_deref() != Some(asset.thumbnail.as_str()) {
        stream.thumb = Some(asset.thumbnail.clone());
        changed = true;
    }
    changed
}

/// Slugify a string for use as a download filename.
/// Keeps alphanumeric, hyphens, and underscores; replaces spaces with hyphens;
/// collapses consecutive hyphens; truncates to 120 characters.
fn slugify_title(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let collapsed = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.len() > 120 {
        collapsed[..120].trim_end_matches('-').to_string()
    } else {
        collapsed
    }
}

/// Build the deterministic MP4 download URL from a video asset webhook.
/// Returns None if the recording exceeds 4 hours (Cloudflare limit for MP4 downloads).
/// When a stream title is provided, appends `?filename=<slugified-title>` so the
/// downloaded file has a meaningful name instead of `default.mp4`.
fn get_download_url(asset: &VideoAssetWebhook, title: Option<&str>) -> Option<String> {
    const MAX_DOWNLOAD_DURATION_SECS: f32 = 4.0 * 60.0 * 60.0; // 4 hours
    if asset.duration > MAX_DOWNLOAD_DURATION_SECS {
        return None;
    }
    // Extract the host from the HLS playback URL (e.g. customer-<hash>.cloudflarestream.com)
    // and construct the download path from the asset UID directly.
    let hls_url = Url::parse(&asset.playback.hls).ok()?;
    let host = hls_url.host_str()?;
    let scheme = hls_url.scheme();
    let mut url = Url::parse(&format!(
        "{}://{}/{}/downloads/default.mp4",
        scheme, host, asset.uid
    ))
    .ok()?;
    if let Some(t) = title {
        let slug = slugify_title(t);
        if !slug.is_empty() {
            url.query_pairs_mut().append_pair("filename", &slug);
        }
    }
    Some(url.to_string())
}

#[derive(Clone, Debug)]
struct ViewerCountCache {
    count: u32,
    timestamp: Instant,
}

#[derive(Clone, Debug)]
struct ViewerCountState {
    last_published_count: u32,
    last_update_time: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
struct ViewerCountTracker {
    cache: Arc<RwLock<HashMap<String, ViewerCountCache>>>,
    cache_duration: Duration,
}

impl ViewerCountTracker {
    fn new(cache_duration: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_duration,
        }
    }

    async fn remove(&self, stream_id: &str) {
        self.cache.write().await.remove(stream_id);
    }

    async fn get_viewer_count(&self, stream_id: &str, hls_url: &str) -> u32 {
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(stream_id) {
                if cached.timestamp.elapsed() < self.cache_duration {
                    return cached.count;
                }
            }
        }

        let viewer_url = hls_url.replace("/manifest/video.m3u8", "/views");
        let response = match reqwest::get(&viewer_url).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("Failed to fetch viewer count from Cloudflare: {}", e);
                return 0;
            }
        };

        let json: serde_json::Value = match response.json().await {
            Ok(j) => j,
            Err(e) => {
                warn!("Failed to parse viewer count JSON: {}", e);
                return 0;
            }
        };

        let count = json["liveViewers"].as_u64().unwrap_or(0) as u32;
        info!(
            "Cloudflare API call: viewer count for stream {}: {}",
            stream_id, count
        );
        let mut cache = self.cache.write().await;
        cache.insert(
            stream_id.to_string(),
            ViewerCountCache {
                count,
                timestamp: Instant::now(),
            },
        );
        count
    }
}

/// Returns `true` if a duplicate `live_input.connected` webhook should be skipped
/// because an active stream already exists for this Cloudflare input.
/// Cloudflare delivers webhooks at-least-once, so duplicates are expected.
fn should_skip_duplicate_webhook(mapped_stream: Option<&UserStream>) -> bool {
    matches!(mapped_stream, Some(s) if s.state == UserStreamState::Live)
}

#[derive(Clone)]
pub struct CfApiWrapper {
    /// Cloudflare API client
    client: CloudflareClient,
    /// Nostr client
    nostr_client: Client,
    /// Internal shared api implementation
    api_base: ApiBase,
    /// Database instance
    db: ZapStreamDb,
    /// Cache of live input data for users
    live_input_cache: Arc<RwLock<HashMap<u64, LiveInput>>>,
    /// Map input uid to stream id with TTL
    input_stream_map: Arc<RwLock<HashMap<String, (String, Instant)>>>,
    /// Terms of Service URL to return in account info
    tos_url: Option<String>,
    /// Client URL for "Watch live on" alt tag
    client_url: String,
    /// Simple mutex to prevent concurrent calls to create live endpoint
    create_input_lock: Arc<Mutex<()>>,
    /// Public hostname which points to our HTTP server
    public_url: String,
    /// Details of the registered webhook
    webhook_details: Arc<RwLock<Option<WebhookResult>>>,
    /// Stream manager tracking active streams
    stream_manager: StreamManager,
    /// Publisher util for nostr events
    n53: N53Publisher,
    /// Viewer count cache with 30-second TTL
    viewer_count_tracker: ViewerCountTracker,
    /// Track viewer count states for change detection
    viewer_count_states: Arc<RwLock<HashMap<String, ViewerCountState>>>,
    /// Minimum update interval in minutes (matches core behavior)
    min_update_minutes: i64,
    /// Custom ingest domain (if configured)
    custom_ingest_domain: Option<String>,
    /// Cached signer public key (static for lifetime of process)
    signer_pubkey: PublicKey,
}

impl CfApiWrapper {
    pub const WEBHOOK_API_PATH: &'static str = "/api/v1/webhook/cloudflare";

    /// Time window (seconds) within which a reconnecting streamer resumes
    /// the previous stream instead of starting a new one.
    /// Matches upstream ZapStreamOverseer::RECONNECT_WINDOW_SECONDS.
    const RECONNECT_WINDOW_SECONDS: u64 = 120;

    pub async fn new(
        token: CloudflareToken,
        db: ZapStreamDb,
        client: Client,
        lightning: Arc<dyn LightningNode>,
        stream_manager: StreamManager,
        public_url: String,
        endpoints_public_hostname: Option<String>,
        tos_url: Option<String>,
        client_url: Option<String>,
    ) -> Result<Self> {
        let signer_pubkey = client.signer().await?.get_public_key().await?;
        Ok(Self {
            client: CloudflareClient::new(token),
            nostr_client: client.clone(),
            api_base: ApiBase::new(db.clone(), client.clone(), lightning),
            db,
            live_input_cache: Default::default(),
            input_stream_map: Default::default(),
            tos_url,
            client_url: resolve_client_url(client_url.as_deref()),
            create_input_lock: Default::default(),
            public_url,
            webhook_details: Default::default(),
            n53: N53Publisher::new(stream_manager.clone(), client.clone()),
            stream_manager,
            viewer_count_tracker: ViewerCountTracker::new(Duration::from_secs(30)),
            viewer_count_states: Default::default(),
            min_update_minutes: 5,
            custom_ingest_domain: endpoints_public_hostname,
            signer_pubkey,
        })
    }

    fn input_map_ttl() -> Duration {
        Duration::from_secs(120)
    }

    async fn register_input_mapping(&self, input_uid: &str, stream_id: &str) {
        let mut map = self.input_stream_map.write().await;
        map.insert(input_uid.to_string(), (stream_id.to_string(), Instant::now()));
    }

    async fn remove_input_mapping(&self, input_uid: &str) {
        let mut map = self.input_stream_map.write().await;
        map.remove(input_uid);
    }

    /// Identify the user and optional stream_key_id from a Cloudflare Live Input.
    /// Returns (User, None) for primary key streams, (User, Some(key_id)) for custom key streams.
    async fn resolve_user_and_key(
        &self,
        input: &LiveInput,
    ) -> Result<(User, Option<u64>)> {
        // Path 1: user.external_id match -> primary key
        if let Some(user) = self.db.get_user_by_external_id(&input.uid).await? {
            return Ok((user, None));
        }

        // Path 2: custom key by external_id (returns full UserStreamKey with .id)
        if let Some(key_row) = self
            .db
            .get_user_stream_key_by_external_id(&input.uid)
            .await?
        {
            let user = self.db.get_user(key_row.user_id).await?;
            return Ok((user, Some(key_row.id)));
        }

        // Path 3: fallback to find_user_stream_key (matches user.stream_key or user_stream_key.key)
        match self.db.find_user_stream_key(&input.uid).await? {
            Some(StreamKeyType::Primary(id)) => {
                let user = self.db.get_user(id).await?;
                Ok((user, None))
            }
            Some(StreamKeyType::FixedEventKey { id, stream_id }) => {
                let user = self.db.get_user(id).await?;
                // find_user_stream_key doesn't return the key row ID, so look it up
                let keys = self.db.get_user_stream_keys(id).await?;
                let key_row = keys
                    .iter()
                    .find(|k| k.stream_id == stream_id)
                    .ok_or_else(|| {
                        anyhow!("Stream key row not found for stream_id {}", stream_id)
                    })?;
                Ok((user, Some(key_row.id)))
            }
            None => bail!("No user found with external_id {}", input.uid),
        }
    }

    /// Resolve the stream to use when a user goes live.
    ///
    /// - **Custom keys**: always reuse the same stream row (UserStreamKey.stream_id),
    ///   matching upstream behavior. The d-tag is the show's persistent identity.
    /// - **Primary keys**: reconnect grace window (120s) resumes a recently-ended stream;
    ///   otherwise creates a new stream with metadata from user defaults.
    async fn resolve_or_create_stream(
        &self,
        user: &User,
        input: &LiveInput,
        stream_key_id: Option<u64>,
    ) -> Result<UserStream> {
        let conn = ConnectionInfo {
            id: Uuid::new_v4(),
            endpoint: "RTMPS".to_string(),
            ip_addr: "".to_string(),
            app_name: "cloudflare".to_string(),
            key: input.rtmps.stream_key.clone(),
        };
        let endpoint = self.detect_endpoint(&conn, user.ingest_id).await?;

        // Custom keys: always reuse the original stream row
        if let Some(key_id) = stream_key_id {
            let keys = self.db.get_user_stream_keys(user.id).await?;
            let key_row = keys
                .iter()
                .find(|k| k.id == key_id)
                .ok_or_else(|| anyhow!("Stream key row not found for id {}", key_id))?;
            let stream_uuid = Uuid::parse_str(&key_row.stream_id)
                .map_err(|e| anyhow!("Invalid stream key UUID {}: {}", key_row.stream_id, e))?;
            let mut stream = self.db.get_stream(&stream_uuid).await?;

            info!(
                "Resuming fixed stream {} for custom key {} (user {})",
                stream.id, key_id, user.id
            );
            stream.state = UserStreamState::Live;
            stream.endpoint_id = Some(endpoint.id);
            stream.ends = None;
            stream.external_input_id = Some(input.uid.clone());
            self.db.update_stream(&stream).await?;
            self.register_input_mapping(&input.uid, &stream.id).await;
            return Ok(stream);
        }

        // Primary keys: reject if already live, check for reconnect grace window
        let prev_streams = self.db.get_user_prev_streams(user.id).await?;

        if prev_streams.live_primary_count > 0 {
            return Err(anyhow!(
                "Primary key is already in use for user {}",
                user.id
            ));
        }

        let has_recent_stream = prev_streams
            .last_ended
            .map(|e| {
                Utc::now()
                    .timestamp()
                    .abs_diff(e.timestamp())
                    < Self::RECONNECT_WINDOW_SECONDS
            })
            .unwrap_or(false);

        if has_recent_stream {
            let prev_id = prev_streams
                .last_stream_id
                .ok_or_else(|| anyhow!("Expected previous stream id not found"))?;
            let stream_uuid = Uuid::parse_str(&prev_id)
                .map_err(|e| anyhow!("Invalid previous stream UUID {}: {}", prev_id, e))?;
            let mut stream = self.db.get_stream(&stream_uuid).await?;

            info!(
                "Resuming previous stream {} for user {} (within {}s grace window)",
                stream.id, user.id, Self::RECONNECT_WINDOW_SECONDS
            );
            stream.state = UserStreamState::Live;
            stream.endpoint_id = Some(endpoint.id);
            stream.ends = None;
            stream.external_input_id = Some(input.uid.clone());
            self.db.update_stream(&stream).await?;
            self.register_input_mapping(&input.uid, &stream.id).await;
            return Ok(stream);
        }

        // No grace window match: create new stream with user defaults
        let new_id = Uuid::new_v4();
        info!(
            "Creating new stream {} for user {} (primary key)",
            new_id, user.id
        );
        let new_stream = UserStream {
            id: new_id.to_string(),
            user_id: user.id,
            starts: Utc::now(),
            state: UserStreamState::Live,
            endpoint_id: Some(endpoint.id),
            title: user.title.clone(),
            summary: user.summary.clone(),
            image: user.image.clone(),
            content_warning: user.content_warning.clone(),
            goal: user.goal.clone(),
            tags: user.tags.clone(),
            stream_key_id: None,
            external_input_id: Some(input.uid.clone()),
            ..Default::default()
        };
        self.db.insert_stream(&new_stream).await?;
        self.register_input_mapping(&input.uid, &new_stream.id).await;

        Ok(new_stream)
    }

    async fn get_mapped_stream(&self, input_uid: &str) -> Result<Option<UserStream>> {
        let stream_uuid = {
            let mut map = self.input_stream_map.write().await;
            let now = Instant::now();
            let ttl = Self::input_map_ttl();
            map.retain(|_, (_, created)| now.duration_since(*created) <= ttl);
            let Some((stream_id, _)) = map.get(input_uid).cloned() else {
                return Ok(None);
            };
            match Uuid::parse_str(&stream_id) {
                Ok(id) => id,
                Err(_) => {
                    map.remove(input_uid);
                    return Ok(None);
                }
            }
        }; // write lock dropped

        let stream = self.db.try_get_stream(&stream_uuid).await?;

        if stream.is_none() {
            self.input_stream_map.write().await.remove(input_uid);
        }

        Ok(stream)
    }

    async fn create_user_live_input(&self, user: &User) -> Result<LiveInput> {
        let _g = self.create_input_lock.lock().await;
        let pk = PublicKey::from_slice(&user.pubkey)?;
        let live_input_name = pk.to_bech32()?;
        let response = self.client.create_live_input(&live_input_name).await?;
        if response.success {
            let response = response.result;
            self.db
                .update_user_external_id(user.id, &response.uid)
                .await?;
            self.live_input_cache
                .write()
                .await
                .insert(user.id, response.clone());

            Ok(response)
        } else {
            bail!("Failed to create live input {:?}", response.errors.first());
        }
    }

    async fn fetch_user_live_input(&self, user: &User) -> Result<LiveInput> {
        let external_id = user.external_id.as_ref().unwrap_or(&user.stream_key);
        let response = self.client.get_live_input(external_id).await?;
        if response.success {
            self.live_input_cache
                .write()
                .await
                .insert(user.id, response.result.clone());
            Ok(response.result)
        } else {
            bail!(
                "Failed to fetch live input, error {:?}",
                response.errors.first()
            );
        }
    }

    async fn get_user_live_input(&self, user: &User) -> Result<LiveInput> {
        let cache = self.live_input_cache.read().await;
        if let Some(input) = cache.get(&user.id) {
            return Ok(input.clone());
        }
        drop(cache);

        // try to load from API next
        // to maintain compat with the backend PR fallback to the stream key if not set
        if let Ok(i) = self.fetch_user_live_input(user).await {
            return Ok(i);
        }

        info!(
            "Creating input for user {}={}",
            user.id,
            hex::encode(&user.pubkey)
        );
        self.create_user_live_input(user).await
    }

    async fn get_user_live_input_by_input_id(&self, input_id: &str) -> Result<LiveInput> {
        let cache = self.live_input_cache.read().await;
        if let Some(input) = cache.values().find(|i| i.uid == input_id) {
            return Ok(input.clone());
        }
        drop(cache);

        // Check primary user external_id first, then fall back to custom key external_id
        let user = if let Some(user) = self.db.get_user_by_external_id(input_id).await? {
            user
        } else if let Some(key) = self.db.get_user_stream_key_by_external_id(input_id).await? {
            self.db.get_user(key.user_id).await?
        } else {
            bail!("No user or stream key found with external_id {}", input_id);
        };

        // try to load from API next
        if let Ok(response) = self.client.get_live_input(input_id).await {
            if response.success {
                self.live_input_cache
                    .write()
                    .await
                    .insert(user.id, response.result.clone());
                return Ok(response.result);
            }
        }

        warn!("Creating input for non-new user {}", user.id);
        self.create_user_live_input(&user).await
    }

    pub fn check_streams(self, token: CancellationToken) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(30));
            loop {
                select! {
                    _ = token.cancelled() => {
                        info!("CF check_streams shutdown");
                        return Ok(());
                    },
                    _ = timer.tick() => {
                       let live_streams = self.db.list_live_streams().await?;
                        info!("Checking {} live streams..", live_streams.len());
                        for live_stream in live_streams {
                            let user = self.db.get_user(live_stream.user_id).await?;
                            let input = if let Some(key_id) = live_stream.stream_key_id {
                                // Custom key (show) — look up the key's Cloudflare input
                                let key = match self.db.get_user_stream_key_by_id(key_id).await {
                                    Ok(k) => k,
                                    Err(e) => {
                                        warn!("Failed to fetch stream key {} for stream {}: {}", key_id, live_stream.id, e);
                                        continue;
                                    }
                                };
                                let external_id = match key.external_id.as_ref() {
                                    Some(id) => id,
                                    None => {
                                        warn!("Stream key {} has no external_id, skipping poll for stream {}", key_id, live_stream.id);
                                        continue;
                                    }
                                };
                                let response = self.client.get_live_input(external_id).await?;
                                if response.success {
                                    response.result
                                } else {
                                    warn!("Failed to fetch live input for stream key {}: {:?}", key_id, response.errors.first());
                                    continue;
                                }
                            } else {
                                // Default key — existing behavior
                                match self.fetch_user_live_input(&user).await {
                                    Ok(r) => r,
                                    Err(e) => {
                                        warn!("Failed to fetch live input for user {}: {}", live_stream.user_id, e);
                                        continue;
                                    }
                                }
                            };
                            if !input.status.as_ref().map(|s| s.is_connected()).unwrap_or(false) {
                                warn!("Database sync issue, live stream {} is supposed to be live but cloudflare input {} shows the status {:?}", live_stream.id, input.uid, input.status);
                                if let Err(e) = self.publish_stream_end(input).await {
                                    warn!("Failed to publish live input for user {}", e);
                                }
                            } else {
                                self.ensure_tracking_live(&input, &user, &live_stream).await?;
                                if let Some(hls_url) = self.get_streaming_url(&live_stream, &input)? {
                                    let viewer_count = self
                                        .viewer_count_tracker
                                        .get_viewer_count(&live_stream.id, &hls_url)
                                        .await;
                                    // Feed viewer count into stream manager so stream_to_event picks it up
                                    self.stream_manager
                                        .set_viewer_count(&live_stream.id, viewer_count as usize)
                                        .await;
                                    if self
                                        .should_publish_viewer_count(&live_stream.id, viewer_count)
                                        .await
                                    {
                                        match self
                                            .publish_stream_event(&live_stream, &user)
                                            .await
                                        {
                                            Ok(event) => {
                                                let mut updated_stream = live_stream.clone();
                                                updated_stream.event = Some(event.as_json());
                                                self.db.update_stream(&updated_stream).await?;
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "Failed to publish viewer count update for stream {}: {}",
                                                    live_stream.id, e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    /// Create a router to handle api requests internally
    pub fn make_router(&self) -> Router {
        Router::new()
            .route(
                Self::WEBHOOK_API_PATH,
                any(
                    async move |headers: HeaderMap, State(this): State<Self>, body: Bytes| {
                        info!(
                            "Got webhook payload: {:?}",
                            str::from_utf8(&body).unwrap_or(&format!("{:?}", body))
                        );
                        if str::from_utf8(&body)
                            .map(|s| s.contains("test message"))
                            .unwrap_or(false)
                        {
                            info!("Webhook test successful!");
                            return Ok(());
                        }
                        // todo: verify sig
                        if headers.get("Webhook-Signature").is_none() {
                            warn!("Webhook signature header missing");
                            //return Err(StatusCode::BAD_REQUEST);
                        }
                        if let Err(e) = this.handle_webhook(body).await {
                            error!("Error handling webhook: {}", e);
                            return Err(StatusCode::INTERNAL_SERVER_ERROR);
                        }
                        Ok(())
                    },
                ),
            )
            .with_state(self.clone())
    }

    async fn handle_webhook(&self, payload: Bytes) -> Result<()> {
        let payload = serde_json::from_slice(&payload)?;
        match payload {
            WebhookPayload::LiveInput(i) => {
                info!(
                    "Received Cloudflare webhook event: {} for input_id: {}",
                    i.data.event_type, i.data.input_id
                );

                // Map Cloudflare event types to our generic events
                match i.data.event_type.as_str() {
                    "live_input.connected" => {
                        let input = self
                            .get_user_live_input_by_input_id(&i.data.input_id)
                            .await?;
                        self.publish_stream_start(input).await?;
                    }
                    "live_input.disconnected" => {
                        let input = self
                            .get_user_live_input_by_input_id(&i.data.input_id)
                            .await?;
                        match self.publish_stream_end(input).await {
                            Ok(()) => {}
                            Err(e) if e.downcast_ref::<NoLiveStreams>().is_some() => {
                                info!("Disconnect webhook received but stream already ended (likely ended by poller): {}", e);
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    t => {
                        warn!("Unknown Cloudflare event type: {}", t);
                    }
                };
            }
            WebhookPayload::VideoAsset(v) => {
                // Only process if the video is ready
                if v.status.state == "ready" {
                    info!(
                        "Cloudflare Video Asset ready for input_uid {}, recording: {} thumbnail: {}",
                        v.live_input, v.playback.hls, v.thumbnail
                    );
                    // Look up the stream that was produced by this CF Live Input.
                    // external_input_id is set at stream start, so this is a direct match.
                    let Some(mut stream) = self.db.get_stream_by_external_input_id(&v.live_input).await? else {
                        warn!(
                            "No stream found for input {}, skipping Video Asset update",
                            v.live_input
                        );
                        return Ok(());
                    };
                    let user = self.db.get_user(stream.user_id).await?;
                    let download_url = get_download_url(&v, stream.title.as_deref());
                    if let Some(ref url) = download_url {
                        let mut enabled = false;
                        for attempt in 1..=3 {
                            match self.client.create_download(&v.uid).await {
                                Ok(_) => {
                                    info!("Enabled MP4 download for video {}: {}", v.uid, url);
                                    enabled = true;
                                    break;
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to enable MP4 download for {} (attempt {}/3): {}",
                                        v.uid, attempt, e
                                    );
                                    if attempt < 3 {
                                        tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
                                    }
                                }
                            }
                        }
                        if !enabled {
                            warn!("Giving up on MP4 download for {} after 3 attempts", v.uid);
                        }
                    }
                    if apply_video_asset_to_stream(&mut stream, &v) {
                        let event = self
                            .publish_stream_event_with_download(&stream, &user, download_url.as_deref())
                            .await?;
                        stream.event = Some(event.as_json());
                        self.db.update_stream(&stream).await?;
                    }
                } else {
                    info!(
                        "Video Asset not ready yet (state: {}), ignoring",
                        v.status.state
                    );
                }
            }
            v => warn!("Unknown Webhook payload: {:?}", v),
        }
        Ok(())
    }

    async fn detect_endpoint(
        &self,
        connection: &ConnectionInfo,
        ingest_id: Option<u64>,
    ) -> Result<IngestEndpoint> {
        // TODO: allow user to select their default endpoint

        let endpoints = self.db.get_ingest_endpoints().await?;
        let selected = select_ingest_endpoint(&endpoints, ingest_id, &connection.app_name)?;
        Ok(selected.clone())
    }

    async fn publish_stream_start(&self, input: LiveInput) -> Result<()> {
        // Dedup guard: skip duplicate live_input.connected webhooks (CF delivers at-least-once)
        let mapped = self.get_mapped_stream(&input.uid).await?;
        if should_skip_duplicate_webhook(mapped.as_ref()) {
            info!(
                "Skipping duplicate live_input.connected webhook for input {}: stream {} is already Live",
                input.uid, mapped.unwrap().id
            );
            return Ok(());
        }

        let (user, stream_key_id) = self.resolve_user_and_key(&input).await?;
        let mut stream = self
            .resolve_or_create_stream(&user, &input, stream_key_id)
            .await?;

        // Publish the "live" event to Nostr. If publish fails, roll back the DB
        // row so we don't leave an orphaned Live stream with no Nostr event.
        match self.publish_stream_event(&stream, &user).await {
            Ok(stream_event) => {
                stream.event = Some(stream_event.as_json());
                self.db.update_stream(&stream).await?;
            }
            Err(e) => {
                warn!(
                    "Failed to publish stream start for stream {}, rolling back: {}",
                    stream.id, e
                );
                stream.state = UserStreamState::Ended;
                stream.ends = Some(Utc::now());
                self.db.update_stream(&stream).await?;
                self.remove_input_mapping(&input.uid).await;
                return Err(e);
            }
        }
        Ok(())
    }

    async fn ensure_tracking_live(
        &self,
        input: &LiveInput,
        user: &User,
        stream: &UserStream,
    ) -> Result<()> {
        let conn = ConnectionInfo {
            id: stream.id.parse()?,
            endpoint: "RTMPS".to_string(),
            ip_addr: "".to_string(),
            app_name: "cloudflare".to_string(),
            key: input.rtmps.stream_key.clone(),
        };
        let streaming = self.get_streaming_url(stream, input)?;
        self.stream_manager
            .add_active_stream(
                &hex::encode(&user.pubkey),
                user.id,
                0.0,   // FPS is never known
                "0x0", // Resolution is never known
                &conn,
                streaming.map(|s| vec![s]).unwrap_or_default(),
                stream.title.as_ref().map(|s| s.as_str()),
            )
            .await;
        Ok(())
    }

    async fn publish_stream_end(&self, input: LiveInput) -> Result<()> {
        let (user, stream_key_id) = self.resolve_user_and_key(&input).await?;

        // Find the live stream matching this key type
        let streams = self.db.get_user_live_streams(user.id).await?;
        let mut stream = streams
            .into_iter()
            .find(|s| s.stream_key_id == stream_key_id)
            .ok_or(NoLiveStreams {
                user_id: user.id,
                stream_key_id,
            })?;

        // Publish the "ended" event to Nostr before mutating any local state.
        // If publish fails, DB still says Live and the poller will retry next cycle.
        stream.state = UserStreamState::Ended;
        stream.ends = Some(Utc::now());
        let event = self.publish_stream_event(&stream, &user).await?;
        stream.event = Some(event.as_json());
        self.db.update_stream(&stream).await?;

        // Event published and DB updated — now clean up local state
        self.stream_manager.remove_active_stream(&stream.id).await;
        zap_stream_core::metrics::remove_playback_rate(&stream.id);

        info!("Stream ended {}", stream.id);
        self.remove_input_mapping(&input.uid).await;
        self.viewer_count_states.write().await.remove(&stream.id);
        self.viewer_count_tracker.remove(&stream.id).await;
        Ok(())
    }

    fn map_to_public_url(&self, path: &str) -> Result<Url> {
        let u: Url = self.public_url.parse()?;
        Ok(u.join(path)?)
    }

    pub fn get_streaming_url(
        &self,
        stream: &UserStream,
        input: &LiveInput,
    ) -> Result<Option<String>> {
        // replace the webrtc url path with the hls/video path
        // for some reason the hls path is not included in the live input details, and we
        // don't have another way to get the hostname part without replacing it with a fixed value
        let mut base_url: Url = input
            .webrtc_playback
            .as_ref()
            .context("webrtc url is missing")?
            .url
            .parse()?;
        match stream.state {
            UserStreamState::Live => {
                base_url.set_path(&format!("{}/manifest/video.m3u8", input.uid));
                return Ok(Some(base_url.to_string()));
            }
            UserStreamState::Ended => {
                // external_video_id is the Cloudflare video/recording UID
                if let Some(r) = &stream.external_video_id {
                    base_url.set_path(&format!("{}/manifest/video.m3u8", r));
                    return Ok(Some(base_url.to_string()));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    async fn publish_stream_event_full(
        &self,
        stream: &UserStream,
        user: &User,
        download_url: Option<&str>,
    ) -> Result<Event> {
        let mut extra_tags = vec![
            Tag::parse(["p", hex::encode(&user.pubkey).as_str(), "", "host"])?,
            Tag::parse(["service", self.map_to_public_url("api/v1")?.as_str()])?,
        ];
        let input = self.get_user_live_input(user).await?;
        match (&stream.state, self.get_streaming_url(&stream, &input)?) {
            (&UserStreamState::Live, Some(u)) => {
                extra_tags.push(Tag::parse(["streaming", u.as_str()])?)
            }
            (&UserStreamState::Ended, Some(u)) => {
                extra_tags.push(Tag::parse(["recording", u.as_str()])?)
            }
            _ => {}
        }
        if let Some(url) = download_url {
            extra_tags.push(Tag::parse(["download", url])?);
        }
        let alt_text = build_alt_text(&self.signer_pubkey, stream, &self.client_url)?;
        let ev = self.n53.stream_to_event(stream, extra_tags, Some(alt_text)).await?;
        self.n53.publish(&ev).await?;
        info!("Published stream event {}", ev.id.to_hex());
        Ok(ev)
    }

    pub async fn publish_stream_event(&self, stream: &UserStream, user: &User) -> Result<Event> {
        self.publish_stream_event_full(stream, user, None)
            .await
    }

    async fn publish_stream_event_with_download(
        &self,
        stream: &UserStream,
        user: &User,
        download_url: Option<&str>,
    ) -> Result<Event> {
        self.publish_stream_event_full(stream, user, download_url)
            .await
    }

    async fn should_publish_viewer_count(&self, stream_id: &str, count: u32) -> bool {
        let mut states = self.viewer_count_states.write().await;
        let now = Utc::now();
        let min_update = chrono::Duration::minutes(self.min_update_minutes);
        match states.get_mut(stream_id) {
            Some(state) => {
                if count == state.last_published_count {
                    return false;
                }
                if now - state.last_update_time < min_update {
                    return false;
                }
                state.last_published_count = count;
                state.last_update_time = now;
                true
            }
            None => {
                states.insert(
                    stream_id.to_string(),
                    ViewerCountState {
                        last_published_count: count,
                        last_update_time: now,
                    },
                );
                true
            }
        }
    }

    /// Register the webhook handler if it's not already set
    pub async fn setup_webhook(&self) -> Result<()> {
        let mut url = Url::parse(&self.public_url)?;
        url.set_path(Self::WEBHOOK_API_PATH);

        // Check if webhook is already registered; treat errors (e.g. 404 on
        // fresh accounts with no webhook) as "not registered" and proceed to
        // create one.
        if let Ok(webhooks) = self.client.get_webhooks().await {
            if webhooks.success {
                if let Some(w) = webhooks.result {
                    if w.notification_url == url.as_str() {
                        info!("Stream webhook already registered: {}", url);
                        self.webhook_details.write().await.replace(w);
                        return Ok(());
                    }
                }
            }
        }

        let wh = self.client.create_webhook(url.to_string().as_str()).await?;
        info!("Stream webhook updated: {}", wh.result.notification_url);
        self.webhook_details.write().await.replace(wh.result);
        Ok(())
    }

    /// Ensure a Cloudflare Alerting notification policy exists for stream live
    /// input events (connected/disconnected). This is a separate system from the
    /// Stream webhook registered by `setup_webhook()`.
    pub async fn setup_notification_policy(&self) -> Result<()> {
        let mut url = Url::parse(&self.public_url)?;
        url.set_path(Self::WEBHOOK_API_PATH);
        let webhook_url = url.to_string();

        // Step 1: Find or create an alerting webhook destination matching our URL
        let destination_id = if let Ok(destinations) =
            self.client.get_alerting_webhook_destinations().await
        {
            if let Some(dest) = destinations
                .result
                .iter()
                .find(|d| d.url.as_deref() == Some(&webhook_url))
            {
                info!(
                    "Notification destination already registered: {}",
                    webhook_url
                );
                dest.id.clone()
            } else {
                let dest = self
                    .client
                    .create_alerting_webhook_destination("ZS Core Webhook", &webhook_url)
                    .await?;
                info!("Notification destination created: {}", webhook_url);
                dest.result.id
            }
        } else {
            let dest = self
                .client
                .create_alerting_webhook_destination("ZS Core Webhook", &webhook_url)
                .await?;
            info!("Notification destination created: {}", webhook_url);
            dest.result.id
        };

        // Step 2: Find or create a notification policy for stream_live_notifications
        // If a policy exists, update it to use our destination (handles switching
        // between environments). If not, create one.
        if let Ok(policies) = self.client.get_alerting_policies().await {
            if let Some(policy) = policies
                .result
                .iter()
                .find(|p| p.alert_type.as_deref() == Some("stream_live_notifications"))
            {
                // Update the existing policy to point to our destination
                self.client
                    .update_alerting_notification_policy(&policy.id, &destination_id)
                    .await?;
                info!("Notification policy updated: {}", webhook_url);
                return Ok(());
            }
        }

        self.client
            .create_alerting_notification_policy("Stream Live Notifications", &destination_id)
            .await?;
        info!("Notification policy created: {}", webhook_url);
        Ok(())
    }
}

#[async_trait]
impl ZapStreamApi for CfApiWrapper {
    async fn get_account(&self, auth: Nip98Auth) -> Result<AccountInfo> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let user = self.db.get_user(uid).await?;
        let input = self.get_user_live_input(&user).await?;

        // Get user forwards
        let forwards = self.db.get_user_forwards(uid).await?;
        let ingests = self.db.get_ingest_endpoints().await?;
        let ingest_endpoint = select_ingest_endpoint(&ingests, user.ingest_id, "")?;
        let endpoints = build_account_endpoints(
            &input,
            ingest_endpoint,
            self.custom_ingest_domain.as_deref(),
        );

        Ok(AccountInfo {
            endpoints,
            balance: user.balance / 1000,
            tos: AccountTos {
                accepted: user.tos_accepted.is_some(),
                link: resolve_tos_url(self.tos_url.as_deref()),
            },
            forwards: forwards
                .into_iter()
                .map(|f| ForwardDest {
                    id: f.id,
                    name: f.name,
                    disabled: f.disabled,
                })
                .collect(),
            details: Some(PatchEventDetails {
                title: user.title,
                summary: user.summary,
                image: user.image,
                tags: user
                    .tags
                    .map(|t| t.split(',').map(|s| s.to_string()).collect()),
                content_warning: user.content_warning,
                goal: user.goal,
            }),
            has_nwc: user.nwc.is_some(),
        })
    }

    async fn update_account(&self, auth: Nip98Auth, patch_account: PatchAccount) -> Result<()> {
        self.api_base.update_account(auth, patch_account).await
    }

    async fn update_event(&self, auth: Nip98Auth, patch: PatchEvent) -> Result<()> {
        self.api_base.update_event(auth, patch).await
    }

    async fn delete_event(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()> {
        self.api_base.delete_event(auth, stream_id).await
    }

    async fn create_forward(
        &self,
        auth: Nip98Auth,
        req: ForwardRequest,
    ) -> Result<ForwardResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let user = self.db.get_user(uid).await?;

        let input = self.get_user_live_input(&user).await?;

        // take stream key off url
        let mut url: Url = req.target.parse()?;
        let tmp_path = url.path().to_string();
        let Some((path, key)) = tmp_path.rsplit_once("/") else {
            bail!("Invalid stream url");
        };
        url.set_path(path);

        let external = self
            .client
            .create_live_input_output(&input.uid, url.as_str(), key)
            .await?;
        if external.success {
            let fwd_id = self
                .db
                .create_forward(uid, &req.name, &req.target, Some(external.result.uid))
                .await?;

            Ok(ForwardResponse { id: fwd_id })
        } else {
            bail!("Failed to create forward entry {:?}", external.errors);
        }
    }

    async fn delete_forward(&self, auth: Nip98Auth, forward_id: u64) -> Result<()> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let Some(fwd) = self.db.get_user_forward(forward_id).await? else {
            bail!("Forward not found");
        };
        if fwd.user_id != uid {
            bail!("Forward not found");
        }

        // delete the forward if it exists on the external system
        if let Some(external) = fwd.external_id {
            let user = self.db.get_user(uid).await?;
            let input = self.get_user_live_input(&user).await?;
            self.client
                .delete_live_input_output(&input.uid, &external)
                .await?;
        }
        self.db.delete_forward(uid, forward_id).await?;
        Ok(())
    }

    async fn update_forward(
        &self,
        auth: Nip98Auth,
        id: u64,
        req: UpdateForwardRequest,
    ) -> Result<ForwardResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let Some(fwd) = self.db.get_user_forward(id).await? else {
            bail!("Forward not found");
        };
        if fwd.user_id != uid {
            bail!("Forward not found");
        }

        // delete the forward if it exists on the external system
        if let Some(external) = fwd.external_id {
            let user = self.db.get_user(uid).await?;
            let input = self.get_user_live_input(&user).await?;
            self.client
                .update_live_input_output(&input.uid, &external, !req.disabled)
                .await?;
        }

        self.db
            .update_forward_disabled(uid, id, req.disabled)
            .await?;
        Ok(ForwardResponse { id })
    }

    async fn get_balance_history(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> Result<HistoryResponse> {
        self.api_base
            .get_balance_history(auth, page, page_size)
            .await
    }

    async fn get_stream_keys(&self, auth: Nip98Auth) -> Result<Vec<StreamKey>> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let keys = self.db.get_user_stream_keys(uid).await?;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(external_id) = key.external_id.as_deref() else {
                out.push(build_stream_key(&key, key.key.clone()));
                continue;
            };
            match self.client.get_live_input(external_id).await {
                Ok(response) => {
                    out.push(build_stream_key(&key, response.result.rtmps.stream_key));
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch live input for key {} (external_id={}): {}",
                        key.id, external_id, e
                    );
                    out.push(build_stream_key(&key, key.key.clone()));
                }
            }
        }
        Ok(out)
    }

    async fn create_stream_key(
        &self,
        auth: Nip98Auth,
        req: CreateStreamKeyRequest,
    ) -> Result<CreateStreamKeyResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let stream_id = Uuid::new_v4();
        let pk = PublicKey::from_slice(&auth.pubkey)?;
        let live_input_name = format!("{}-{}", pk.to_bech32()?, stream_id);
        let response = self.client.create_live_input(&live_input_name).await?;
        let input = response.result;
        // If DB writes below fail, this CF Live Input becomes a "ghost" — it exists
        // on Cloudflare but has no DB reference. This is intentional: ghost inputs are
        // harmless (no quota cost, no billing), and deleting CF inputs risks breaking
        // active stream key references that cannot be recovered.

        let mut new_stream = zap_stream_db::UserStream {
            id: stream_id.to_string(),
            user_id: uid,
            starts: Utc::now(),
            state: zap_stream_db::UserStreamState::Planned,
            title: req.event.title,
            summary: req.event.summary,
            image: req.event.image,
            tags: req.event.tags.map(|t| t.join(",")),
            content_warning: req.event.content_warning,
            goal: req.event.goal,
            ..Default::default()
        };

        self.db.insert_stream(&new_stream).await?;

        let key_id = self
            .db
            .create_stream_key(
                uid,
                &input.rtmps.stream_key,
                Some(&input.uid),
                req.expires,
                &stream_id.to_string(),
            )
            .await?;

        new_stream.stream_key_id = Some(key_id);
        self.db.update_stream(&new_stream).await?;

        Ok(CreateStreamKeyResponse {
            key: input.rtmps.stream_key,
            event: None,
        })
    }

    async fn delete_stream_key(&self, _auth: Nip98Auth, _key_id: u64) -> Result<()> {
        bail!("Not supported");
    }

    async fn topup(
        &self,
        pubkey: [u8; 32],
        amount: u64,
        zap: Option<String>,
    ) -> Result<TopupResponse> {
        self.api_base.topup(pubkey, amount, zap).await
    }

    async fn search_games(&self, _q: String) -> Result<Vec<GameInfo>> {
        todo!()
    }

    async fn get_game(&self, _id: String) -> Result<GameInfo> {
        todo!()
    }
}

fn resolve_tos_url(tos_url: Option<&str>) -> String {
    match tos_url {
        Some(url) if !url.trim().is_empty() => url.to_string(),
        _ => "https://zap.stream/tos".to_string(),
    }
}

fn resolve_client_url(client_url: Option<&str>) -> String {
    match client_url {
        Some(url) if !url.trim().is_empty() => url.trim_end_matches('/').to_string(),
        _ => "https://zap.stream".to_string(),
    }
}

fn build_alt_text(pubkey: &PublicKey, stream: &UserStream, client_url: &str) -> Result<String> {
    let coord = Coordinate::new(Kind::LiveEvent, *pubkey).identifier(&stream.id);
    Ok(format!(
        "Watch live on {}/{}",
        client_url,
        nostr_sdk::nips::nip19::Nip19Coordinate {
            coordinate: coord,
            relays: vec![]
        }
        .to_bech32()?
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        apply_custom_ingest_domain, apply_video_asset_to_stream, build_account_endpoints,
        build_alt_text, build_stream_key, get_download_url, resolve_client_url, resolve_tos_url,
        select_ingest_endpoint, should_skip_duplicate_webhook,
        slugify_title, ViewerCountTracker,
    };
    use crate::cloudflare::{LiveInput, Playback, RtmpsEndpoint, SrtEndpoint, VideoAssetStatus, VideoAssetWebhook};
    use mockito::Server;
    use std::time::Duration;
    use nostr_sdk::Keys;
    use chrono::Utc;
    use zap_stream_db::IngestEndpoint;
    use zap_stream_db::UserStreamKey;
    use zap_stream_db::UserStream;
    use zap_stream_db::UserStreamState;

    fn sample_ingests() -> Vec<IngestEndpoint> {
        vec![
            IngestEndpoint {
                id: 1,
                name: "Good".to_string(),
                cost: 2500,
                capabilities: Some(
                    "variant:2160:20000000,variant:1440:12000000".to_string(),
                ),
            },
            IngestEndpoint {
                id: 2,
                name: "Basic".to_string(),
                cost: 0,
                capabilities: Some("variant:source".to_string()),
            },
        ]
    }

    fn sample_input() -> LiveInput {
        LiveInput {
            uid: "input-uid".to_string(),
            rtmps: RtmpsEndpoint {
                url: "rtmps://live.cloudflare.com:443/live/".to_string(),
                stream_key: "stream-key".to_string(),
            },
            rtmps_playback: None,
            srt: None,
            srt_playback: None,
            webrtc: None,
            webrtc_playback: None,
            status: None,
            created: "2025-01-01T00:00:00Z".to_string(),
            modified: None,
            meta: None,
            recording: None,
            delete_recording_after_days: None,
        }
    }

    fn sample_stream() -> UserStream {
        UserStream {
            id: "stream-id".to_string(),
            user_id: 1,
            starts: chrono::Utc::now(),
            ends: None,
            state: UserStreamState::Planned,
            title: None,
            summary: None,
            image: None,
            thumb: None,
            tags: None,
            content_warning: None,
            goal: None,
            pinned: None,
            cost: 0,
            duration: 0.0,
            fee: None,
            event: None,
            endpoint_id: None,
            node_name: None,
            stream_key_id: None,
            external_video_id: None,
            external_input_id: None,
        }
    }

    #[test]
    fn select_ingest_endpoint_prefers_ingest_id_then_app_name() {
        let endpoints = sample_ingests();
        let matched = select_ingest_endpoint(&endpoints, Some(2), "basic").unwrap();
        assert_eq!(matched.id, 2);

        let fallback = select_ingest_endpoint(&endpoints, None, "cloudflare").unwrap();
        assert_eq!(fallback.id, 2);
    }

    #[test]
    fn build_account_endpoints_uses_ingest_cost() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let endpoints = build_account_endpoints(&sample_input(), &ingest, None);

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].name, "RTMPS-Basic");
        assert_eq!(endpoints[0].cost.rate, 0.0);
        assert_eq!(endpoints[0].key, "stream-key");
    }

    #[test]
    fn build_account_endpoints_applies_custom_ingest_domain() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let endpoints = build_account_endpoints(&sample_input(), &ingest, Some("custom.domain"));

        assert_eq!(endpoints[0].url, "rtmps://custom.domain:443/live/");
    }

    #[test]
    fn build_account_endpoints_ignores_srt_when_disabled() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let mut input = sample_input();
        input.srt = Some(SrtEndpoint {
            url: "srt://live.cloudflare.com:778".to_string(),
            stream_id: "test-stream-id".to_string(),
            passphrase: "test-passphrase".to_string(),
        });

        let endpoints = build_account_endpoints(&input, &ingest, None);

        // SRT endpoint is currently disabled (commented out in build_account_endpoints)
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].name, "RTMPS-Basic");
    }

    #[test]
    fn build_account_endpoints_omits_srt_when_absent() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let endpoints = build_account_endpoints(&sample_input(), &ingest, None);

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].name, "RTMPS-Basic");
    }

    #[test]
    fn build_account_endpoints_applies_custom_domain_to_rtmps() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let mut input = sample_input();
        input.srt = Some(SrtEndpoint {
            url: "srt://live.cloudflare.com:778".to_string(),
            stream_id: "test-stream-id".to_string(),
            passphrase: "test-passphrase".to_string(),
        });

        let endpoints = build_account_endpoints(&input, &ingest, Some("custom.domain"));

        // SRT endpoint is currently disabled, only RTMPS with custom domain
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].url, "rtmps://custom.domain:443/live/");
    }

    #[test]
    fn build_stream_key_uses_provided_value() {
        let created = Utc::now();
        let expires = created + chrono::Duration::hours(1);
        let row = UserStreamKey {
            id: 10,
            user_id: 1,
            key: "input-uid".to_string(),
            external_id: Some("cf-uid".to_string()),
            created,
            expires: Some(expires),
            stream_id: "stream-id".to_string(),
        };
        let mapped = build_stream_key(&row, "stream-key".to_string());

        assert_eq!(mapped.id, 10);
        assert_eq!(mapped.key, "stream-key");
        assert_eq!(mapped.created, created.timestamp());
        assert_eq!(mapped.expires, Some(expires.timestamp()));
        assert_eq!(mapped.stream_id, "stream-id");
    }

    #[test]
    fn apply_custom_ingest_domain_ignores_empty_or_localhost() {
        let input = sample_input();
        assert_eq!(
            apply_custom_ingest_domain(&input.rtmps.url, None),
            "rtmps://live.cloudflare.com:443/live/"
        );
        assert_eq!(
            apply_custom_ingest_domain(&input.rtmps.url, Some("")),
            "rtmps://live.cloudflare.com:443/live/"
        );
        assert_eq!(
            apply_custom_ingest_domain(&input.rtmps.url, Some("localhost")),
            "rtmps://live.cloudflare.com:443/live/"
        );
    }

    #[test]
    fn resolve_tos_url_prefers_configured_value() {
        assert_eq!(
            resolve_tos_url(Some("https://tos.example")),
            "https://tos.example"
        );
        assert_eq!(
            resolve_tos_url(None),
            "https://zap.stream/tos"
        );
    }

    #[test]
    fn resolve_client_url_prefers_configured_value() {
        assert_eq!(
            resolve_client_url(Some("https://client.example")),
            "https://client.example"
        );
        assert_eq!(
            resolve_client_url(Some("https://client.example/")),
            "https://client.example"
        );
        assert_eq!(
            resolve_client_url(None),
            "https://zap.stream"
        );
    }

    #[test]
    fn build_alt_text_uses_configured_client_url() {
        let keys = Keys::generate();
        let alt_text = build_alt_text(&keys.public_key(), &sample_stream(), "https://client.example")
            .unwrap();

        assert!(alt_text.starts_with("Watch live on https://client.example/"));
    }

    #[test]
    fn apply_video_asset_to_stream_updates_fields() {
        let mut stream = UserStream::default();
        let asset = VideoAssetWebhook {
            uid: "video-uid".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 12.0,
            playback: Playback {
                hls: "https://example.com/video.m3u8".to_string(),
                dash: "https://example.com/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let changed = apply_video_asset_to_stream(&mut stream, &asset);
        assert!(changed);
        assert_eq!(stream.external_video_id.as_deref(), Some("video-uid"));
        assert_eq!(
            stream.thumb.as_deref(),
            Some("https://example.com/thumb.jpg")
        );
    }

    #[test]
    fn apply_video_asset_to_stream_is_idempotent() {
        let mut stream = UserStream {
            external_video_id: Some("video-uid".to_string()),
            thumb: Some("https://example.com/thumb.jpg".to_string()),
            ..Default::default()
        };
        let asset = VideoAssetWebhook {
            uid: "video-uid".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 12.0,
            playback: Playback {
                hls: "https://example.com/video.m3u8".to_string(),
                dash: "https://example.com/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let changed = apply_video_asset_to_stream(&mut stream, &asset);
        assert!(!changed);
    }

    #[test]
    fn apply_video_asset_to_stream_updates_only_external_video_id() {
        let mut stream = UserStream {
            external_video_id: Some("old-uid".to_string()),
            thumb: Some("https://example.com/thumb.jpg".to_string()),
            ..Default::default()
        };
        let asset = VideoAssetWebhook {
            uid: "video-uid".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 12.0,
            playback: Playback {
                hls: "https://example.com/video.m3u8".to_string(),
                dash: "https://example.com/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let changed = apply_video_asset_to_stream(&mut stream, &asset);
        assert!(changed);
        assert_eq!(stream.external_video_id.as_deref(), Some("video-uid"));
        assert_eq!(
            stream.thumb.as_deref(),
            Some("https://example.com/thumb.jpg")
        );
    }

    #[test]
    fn apply_video_asset_to_stream_updates_only_thumb() {
        let mut stream = UserStream {
            external_video_id: Some("video-uid".to_string()),
            thumb: Some("https://example.com/old.jpg".to_string()),
            ..Default::default()
        };
        let asset = VideoAssetWebhook {
            uid: "video-uid".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 12.0,
            playback: Playback {
                hls: "https://example.com/video.m3u8".to_string(),
                dash: "https://example.com/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let changed = apply_video_asset_to_stream(&mut stream, &asset);
        assert!(changed);
        assert_eq!(stream.external_video_id.as_deref(), Some("video-uid"));
        assert_eq!(
            stream.thumb.as_deref(),
            Some("https://example.com/thumb.jpg")
        );
    }

    #[test]
    fn get_download_url_constructs_mp4_path() {
        let asset = VideoAssetWebhook {
            uid: "video-uid-abc".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 600.0, // 10 minutes
            playback: Playback {
                hls: "https://customer-test.cloudflarestream.com/video-uid-abc/manifest/video.m3u8".to_string(),
                dash: "https://customer-test.cloudflarestream.com/video-uid-abc/manifest/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let url = get_download_url(&asset, None);
        assert_eq!(
            url,
            Some("https://customer-test.cloudflarestream.com/video-uid-abc/downloads/default.mp4".to_string())
        );
    }

    #[test]
    fn get_download_url_with_title_appends_filename() {
        let asset = VideoAssetWebhook {
            uid: "video-uid-abc".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 600.0,
            playback: Playback {
                hls: "https://customer-test.cloudflarestream.com/video-uid-abc/manifest/video.m3u8".to_string(),
                dash: "https://customer-test.cloudflarestream.com/video-uid-abc/manifest/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let url = get_download_url(&asset, Some("My Cool Stream!"));
        assert_eq!(
            url,
            Some("https://customer-test.cloudflarestream.com/video-uid-abc/downloads/default.mp4?filename=my-cool-stream".to_string())
        );
    }

    #[test]
    fn get_download_url_skips_recordings_over_4_hours() {
        let asset = VideoAssetWebhook {
            uid: "video-uid-long".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 14401.0, // just over 4 hours
            playback: Playback {
                hls: "https://customer-test.cloudflarestream.com/video-uid-long/manifest/video.m3u8".to_string(),
                dash: "https://customer-test.cloudflarestream.com/video-uid-long/manifest/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let url = get_download_url(&asset, None);
        assert_eq!(url, None);
    }

    #[test]
    fn get_download_url_allows_exactly_4_hours() {
        let asset = VideoAssetWebhook {
            uid: "video-uid-exact".to_string(),
            thumbnail: "https://example.com/thumb.jpg".to_string(),
            duration: 14400.0, // exactly 4 hours
            playback: Playback {
                hls: "https://customer-test.cloudflarestream.com/video-uid-exact/manifest/video.m3u8".to_string(),
                dash: "https://customer-test.cloudflarestream.com/video-uid-exact/manifest/video.mpd".to_string(),
            },
            live_input: "input-uid".to_string(),
            status: VideoAssetStatus {
                state: "ready".to_string(),
            },
        };

        let url = get_download_url(&asset, Some("Exactly 4h Stream"));
        assert_eq!(
            url,
            Some("https://customer-test.cloudflarestream.com/video-uid-exact/downloads/default.mp4?filename=exactly-4h-stream".to_string())
        );
    }

    #[test]
    fn slugify_title_handles_special_characters() {
        assert_eq!(slugify_title("Hello World!"), "hello-world");
        assert_eq!(slugify_title("My  Stream -- Live"), "my-stream-live");
        assert_eq!(slugify_title("test"), "test");
        assert_eq!(slugify_title(""), "");
        assert_eq!(slugify_title("café & más"), "caf-m-s");
        assert_eq!(slugify_title("under_score test"), "under_score-test");
    }

    #[tokio::test]
    async fn viewer_count_cache() {
        let mut server = Server::new_async().await;
        let views_path = "/stream-uid/views";
        let mock = server
            .mock("GET", views_path)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"liveViewers": 12}"#)
            .expect(1)
            .create_async()
            .await;
        let hls_url = format!("{}/stream-uid/manifest/video.m3u8", server.url());
        let tracker = ViewerCountTracker::new(Duration::from_secs(30));

        let first = tracker.get_viewer_count("stream-1", &hls_url).await;
        let second = tracker.get_viewer_count("stream-1", &hls_url).await;

        assert!(first > 0);
        assert_eq!(first, second);
        mock.assert_async().await;
    }

    #[test]
    fn skip_duplicate_when_stream_is_live() {
        let stream = UserStream {
            state: UserStreamState::Live,
            ..sample_stream()
        };
        assert!(should_skip_duplicate_webhook(Some(&stream)));
    }

    #[test]
    fn allow_webhook_when_no_mapped_stream() {
        assert!(!should_skip_duplicate_webhook(None));
    }

    #[test]
    fn allow_webhook_when_mapped_stream_ended() {
        let stream = UserStream {
            state: UserStreamState::Ended,
            ..sample_stream()
        };
        assert!(!should_skip_duplicate_webhook(Some(&stream)));
    }

    #[test]
    fn allow_webhook_when_mapped_stream_planned() {
        let stream = UserStream {
            state: UserStreamState::Planned,
            ..sample_stream()
        };
        assert!(!should_skip_duplicate_webhook(Some(&stream)));
    }
}
