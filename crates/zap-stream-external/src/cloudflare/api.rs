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
use nostr_sdk::{Client, Event, JsonUtil, Kind, NostrSigner, PublicKey, Tag, ToBech32};
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

    if let Some(srt) = &input.srt {
        endpoints.push(Endpoint {
            name: format!("SRT-{}", ingest.name),
            url: apply_custom_ingest_domain(&srt.url, custom_domain),
            key: format!("streamid={}&passphrase={}", srt.stream_id, srt.passphrase),
            capabilities: vec![],
            cost: EndpointCost {
                unit: "min".to_string(),
                rate: ingest.cost as f32 / 1000.0,
            },
        });
    }

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
    if stream.external_id.as_deref() != Some(asset.uid.as_str()) {
        stream.external_id = Some(asset.uid.clone());
        changed = true;
    }
    if stream.thumb.as_deref() != Some(asset.thumbnail.as_str()) {
        stream.thumb = Some(asset.thumbnail.clone());
        changed = true;
    }
    changed
}

fn select_stream_for_video_asset(
    matched: Option<UserStream>,
    fallback: Option<UserStream>,
) -> Option<UserStream> {
    matched.or(fallback)
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
}

impl CfApiWrapper {
    pub const WEBHOOK_API_PATH: &'static str = "/api/v1/webhook/cloudflare";

    pub fn new(
        token: CloudflareToken,
        db: ZapStreamDb,
        client: Client,
        lightning: Arc<dyn LightningNode>,
        stream_manager: StreamManager,
        public_url: String,
        endpoints_public_hostname: Option<String>,
        tos_url: Option<String>,
        client_url: Option<String>,
    ) -> Self {
        Self {
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
        }
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

    async fn get_mapped_stream(&self, input_uid: &str) -> Result<Option<UserStream>> {
        let mut map = self.input_stream_map.write().await;
        let now = Instant::now();
        let ttl = Self::input_map_ttl();
        map.retain(|_, (_, created)| now.duration_since(*created) <= ttl);
        let Some((stream_id, _)) = map.get(input_uid).cloned() else {
            return Ok(None);
        };
        let stream_uuid = match Uuid::parse_str(&stream_id) {
            Ok(id) => id,
            Err(_) => {
                map.remove(input_uid);
                return Ok(None);
            }
        };
        let stream = self.db.try_get_stream(&stream_uuid).await?;
        if stream.is_none() {
            map.remove(input_uid);
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
                            let input = match self.fetch_user_live_input(&user).await {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!("Failed to fetch live input for user {}: {}", live_stream.user_id, e);
                                    continue;
                                }
                            };
                            if !input.status.as_ref().map(|s| s.is_connected()).unwrap_or(false) {
                                warn!("Database sync issue, live stream is supposed to be live but cloudflare shows the status {:?}", input.status);
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
                                    if self
                                        .should_publish_viewer_count(&live_stream.id, viewer_count)
                                        .await
                                    {
                                        let event = self
                                            .publish_stream_event_with_viewer_count(
                                                &live_stream,
                                                &user,
                                                Some(viewer_count),
                                            )
                                            .await?;
                                        let mut updated_stream = live_stream.clone();
                                        updated_stream.event = Some(event.as_json());
                                        self.db.update_stream(&updated_stream).await?;
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
                        self.publish_stream_end(input).await?;
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
                    let input = self.get_user_live_input_by_input_id(&v.live_input).await?;
                    let user = if let Some(user) = self.db.get_user_by_external_id(&input.uid).await? {
                        user
                    } else if let Some(key) = self.db.get_user_stream_key_by_external_id(&input.uid).await? {
                        self.db.get_user(key.user_id).await?
                    } else {
                        bail!("No user or stream key found with external_id {}", input.uid);
                    };
                    let matched = self.get_mapped_stream(&v.live_input).await?;
                    let fallback = self.db.get_user_latest_ended_stream(user.id).await?;
                    let Some(mut stream) =
                        select_stream_for_video_asset(matched, fallback)
                    else {
                        warn!(
                            "No ended streams found for user {}, skipping Video Asset update",
                            user.id
                        );
                        return Ok(());
                    };
                    let user = if stream.user_id == user.id {
                        user
                    } else {
                        self.db.get_user(stream.user_id).await?
                    };
                    if apply_video_asset_to_stream(&mut stream, &v) {
                        let event = self.publish_stream_event(&stream, &user).await?;
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
        let user = self.db.get_user_by_external_id(&input.uid).await?;
        if let Some(user) = user {
            let new_id = Uuid::new_v4();
            let conn = ConnectionInfo {
                id: new_id,
                endpoint: "RTMPS".to_string(),
                ip_addr: "".to_string(),
                app_name: "cloudflare".to_string(),
                key: input.rtmps.stream_key.clone(),
            };
            let endpoint = self.detect_endpoint(&conn, user.ingest_id).await?;
            // start a new stream
            let mut new_stream = UserStream {
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
                ..Default::default()
            };
            self.db.insert_stream(&new_stream).await?;
            self.register_input_mapping(&input.uid, &new_stream.id).await;

            let stream_event = self.publish_stream_event(&new_stream, &user).await?;
            new_stream.event = Some(stream_event.as_json());
            self.db.update_stream(&new_stream).await?;
            return Ok(());
        }

        let user_key = if let Some(row) = self
            .db
            .get_user_stream_key_by_external_id(&input.uid)
            .await?
        {
            StreamKeyType::FixedEventKey {
                id: row.user_id,
                stream_id: row.stream_id,
            }
        } else {
            self.db.find_user_stream_key(&input.uid).await?
                .ok_or_else(|| anyhow!("No user found with external_id {}", input.uid))?
        };

        let (user, stream_uuid) = match user_key {
            StreamKeyType::Primary(id) => {
                let user = self.db.get_user(id).await?;
                let new_id = Uuid::new_v4();
                let conn = ConnectionInfo {
                    id: new_id,
                    endpoint: "RTMPS".to_string(),
                    ip_addr: "".to_string(),
                    app_name: "cloudflare".to_string(),
                    key: input.rtmps.stream_key.clone(),
                };
                let endpoint = self.detect_endpoint(&conn, user.ingest_id).await?;
                let mut new_stream = UserStream {
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
                    ..Default::default()
                };
                self.db.insert_stream(&new_stream).await?;
                self.register_input_mapping(&input.uid, &new_stream.id).await;

                let stream_event = self.publish_stream_event(&new_stream, &user).await?;
                new_stream.event = Some(stream_event.as_json());
                self.db.update_stream(&new_stream).await?;
                return Ok(());
            }
            StreamKeyType::FixedEventKey { id, stream_id } => {
                let user = self.db.get_user(id).await?;
                let stream_uuid = Uuid::parse_str(&stream_id)?;
                (user, stream_uuid)
            }
        };

        let mut stream = self.db.get_stream(&stream_uuid).await?;
        let conn = ConnectionInfo {
            id: stream_uuid,
            endpoint: "RTMPS".to_string(),
            ip_addr: "".to_string(),
            app_name: "cloudflare".to_string(),
            key: input.rtmps.stream_key.clone(),
        };
        let endpoint = self.detect_endpoint(&conn, user.ingest_id).await?;

        stream.state = UserStreamState::Live;
        stream.endpoint_id = Some(endpoint.id);
        stream.ends = None;
        self.db.update_stream(&stream).await?;
        self.register_input_mapping(&input.uid, &stream.id).await;

        let stream_event = self.publish_stream_event(&stream, &user).await?;
        stream.event = Some(stream_event.as_json());
        self.db.update_stream(&stream).await?;
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
        let user = self.db.get_user_by_external_id(&input.uid).await?;
        if let Some(user) = user {
            let streams = self.db.get_user_live_streams(user.id).await?;
            let Some(mut stream) = streams.into_iter().next() else {
                bail!("No live streams found for user {}", user.id);
            };

            self.stream_manager.remove_active_stream(&stream.id).await;
            zap_stream_core::metrics::remove_playback_rate(&stream.id);

            stream.state = UserStreamState::Ended;
            stream.ends = Some(Utc::now());
            let event = self.publish_stream_event(&stream, &user).await?;
            stream.event = Some(event.as_json());
            self.db.update_stream(&stream).await?;

            info!("Stream ended {}", stream.id);
            self.remove_input_mapping(&input.uid).await;
            return Ok(());
        }

        let user_key = if let Some(row) = self
            .db
            .get_user_stream_key_by_external_id(&input.uid)
            .await?
        {
            StreamKeyType::FixedEventKey {
                id: row.user_id,
                stream_id: row.stream_id,
            }
        } else {
            self.db.find_user_stream_key(&input.uid).await?
                .ok_or_else(|| anyhow!("No user found with external_id {}", input.uid))?
        };

        let (user, stream_uuid) = match user_key {
            StreamKeyType::Primary(id) => {
                let user = self.db.get_user(id).await?;
                let streams = self.db.get_user_live_streams(user.id).await?;
                let Some(mut stream) = streams.into_iter().next() else {
                    bail!("No live streams found for user {}", user.id);
                };

                self.stream_manager.remove_active_stream(&stream.id).await;
                zap_stream_core::metrics::remove_playback_rate(&stream.id);

                stream.state = UserStreamState::Ended;
                stream.ends = Some(Utc::now());
                let event = self.publish_stream_event(&stream, &user).await?;
                stream.event = Some(event.as_json());
                self.db.update_stream(&stream).await?;

                info!("Stream ended {}", stream.id);
                self.remove_input_mapping(&input.uid).await;
                return Ok(());
            }
            StreamKeyType::FixedEventKey { id, stream_id } => {
                let user = self.db.get_user(id).await?;
                let stream_uuid = Uuid::parse_str(&stream_id)?;
                (user, stream_uuid)
            }
        };

        let mut stream = self.db.get_stream(&stream_uuid).await?;

        self.stream_manager.remove_active_stream(&stream.id).await;
        zap_stream_core::metrics::remove_playback_rate(&stream.id);

        stream.state = UserStreamState::Ended;
        stream.ends = Some(Utc::now());
        let event = self.publish_stream_event(&stream, &user).await?;
        stream.event = Some(event.as_json());
        self.db.update_stream(&stream).await?;

        info!("Stream ended {}", stream.id);
        self.remove_input_mapping(&input.uid).await;
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
                // external_id will be the video id
                if let Some(r) = &stream.external_id {
                    base_url.set_path(&format!("{}/manifest/video.m3u8", r));
                    return Ok(Some(base_url.to_string()));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    async fn publish_stream_event_with_viewer_count(
        &self,
        stream: &UserStream,
        user: &User,
        viewer_count: Option<u32>,
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
        if let Some(count) = viewer_count {
            let count_str = count.to_string();
            extra_tags.push(Tag::parse(["current_participants", count_str.as_str()])?);
        }
        let alt_tag = build_alt_tag(&self.nostr_client, stream, &self.client_url).await?;
        extra_tags.push(alt_tag);
        let ev = self.n53.stream_to_event(stream, extra_tags).await?;
        self.n53.publish(&ev).await?;
        info!("Published stream event {}", ev.id.to_hex());
        Ok(ev)
    }

    pub async fn publish_stream_event(&self, stream: &UserStream, user: &User) -> Result<Event> {
        self.publish_stream_event_with_viewer_count(stream, user, None)
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

        let webhooks = self.client.get_webhooks().await?;
        if webhooks.success
            && let Some(w) = webhooks.result
            && w.notification_url == url.as_str()
        {
            info!("Webhook notification url already registered: {}", url);
            self.webhook_details.write().await.replace(w);
            return Ok(());
        }

        let wh = self.client.create_webhook(url.to_string().as_str()).await?;
        info!("Webhook created for {}", wh.result.notification_url);
        self.webhook_details.write().await.replace(wh.result);
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

async fn build_alt_tag(client: &Client, stream: &UserStream, client_url: &str) -> Result<Tag> {
    let pubkey = client.signer().await?.get_public_key().await?;
    let coord = Coordinate::new(Kind::LiveEvent, pubkey).identifier(&stream.id);
    Tag::parse([
        "alt",
        &format!(
            "Watch live on {}/{}",
            client_url,
            nostr_sdk::nips::nip19::Nip19Coordinate {
                coordinate: coord,
                relays: vec![]
            }
            .to_bech32()?
        ),
    ])
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_custom_ingest_domain, apply_video_asset_to_stream, build_account_endpoints,
        build_alt_tag, build_stream_key, resolve_client_url, resolve_tos_url,
        select_ingest_endpoint, select_stream_for_video_asset, ViewerCountTracker,
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
            external_id: None,
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
    fn build_account_endpoints_includes_srt_when_available() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let mut input = sample_input();
        input.srt = Some(SrtEndpoint {
            url: "srt://live.cloudflare.com:778".to_string(),
            stream_id: "test-stream-id".to_string(),
            passphrase: "test-passphrase".to_string(),
        });

        let endpoints = build_account_endpoints(&input, &ingest, None);

        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].name, "RTMPS-Basic");
        assert_eq!(endpoints[1].name, "SRT-Basic");
        assert_eq!(endpoints[1].url, "srt://live.cloudflare.com:778");
        assert_eq!(
            endpoints[1].key,
            "streamid=test-stream-id&passphrase=test-passphrase"
        );
        assert_eq!(endpoints[0].cost.rate, endpoints[1].cost.rate);
    }

    #[test]
    fn build_account_endpoints_omits_srt_when_absent() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let endpoints = build_account_endpoints(&sample_input(), &ingest, None);

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].name, "RTMPS-Basic");
    }

    #[test]
    fn build_account_endpoints_applies_custom_domain_to_srt() {
        let ingest = sample_ingests().into_iter().find(|e| e.id == 2).unwrap();
        let mut input = sample_input();
        input.srt = Some(SrtEndpoint {
            url: "srt://live.cloudflare.com:778".to_string(),
            stream_id: "test-stream-id".to_string(),
            passphrase: "test-passphrase".to_string(),
        });

        let endpoints = build_account_endpoints(&input, &ingest, Some("custom.domain"));

        assert_eq!(endpoints[0].url, "rtmps://custom.domain:443/live/");
        assert_eq!(endpoints[1].url, "srt://custom.domain:778");
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

    #[tokio::test]
    async fn build_nostr_event_uses_configured_client_url() {
        let keys = Keys::generate();
        let client = nostr_sdk::ClientBuilder::new().signer(keys).build();
        let alt_tag = build_alt_tag(&client, &sample_stream(), "https://client.example")
            .await
            .unwrap();
        let alt_value = alt_tag
            .as_slice()
            .get(1)
            .map(|v| v.as_str())
            .unwrap();

        assert!(alt_value.starts_with("Watch live on https://client.example/"));
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
        assert_eq!(stream.external_id.as_deref(), Some("video-uid"));
        assert_eq!(
            stream.thumb.as_deref(),
            Some("https://example.com/thumb.jpg")
        );
    }

    #[test]
    fn apply_video_asset_to_stream_is_idempotent() {
        let mut stream = UserStream {
            external_id: Some("video-uid".to_string()),
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
    fn apply_video_asset_to_stream_updates_only_external_id() {
        let mut stream = UserStream {
            external_id: Some("old-uid".to_string()),
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
        assert_eq!(stream.external_id.as_deref(), Some("video-uid"));
        assert_eq!(
            stream.thumb.as_deref(),
            Some("https://example.com/thumb.jpg")
        );
    }

    #[test]
    fn apply_video_asset_to_stream_updates_only_thumb() {
        let mut stream = UserStream {
            external_id: Some("video-uid".to_string()),
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
        assert_eq!(stream.external_id.as_deref(), Some("video-uid"));
        assert_eq!(
            stream.thumb.as_deref(),
            Some("https://example.com/thumb.jpg")
        );
    }

    #[test]
    fn select_stream_for_video_asset_prefers_matched() {
        let matched = UserStream {
            id: "matched".to_string(),
            ..Default::default()
        };
        let fallback = UserStream {
            id: "fallback".to_string(),
            ..Default::default()
        };

        let selected = select_stream_for_video_asset(Some(matched), Some(fallback)).unwrap();
        assert_eq!(selected.id, "matched");
    }

    #[test]
    fn select_stream_for_video_asset_uses_fallback_when_missing() {
        let fallback = UserStream {
            id: "fallback".to_string(),
            ..Default::default()
        };

        let selected = select_stream_for_video_asset(None, Some(fallback)).unwrap();
        assert_eq!(selected.id, "fallback");
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
}
