use crate::cloudflare::{
    CloudflareClient, CloudflareToken, LiveInput, LiveInputWebhookData,
    WebhookPayload, WebhookResult,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{any};
use axum::{Router};
use chrono::Utc;
use nostr_sdk::{Client, Event, JsonUtil, PublicKey, Tag, ToBech32};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
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
use zap_stream_db::{IngestEndpoint, User, UserStream, UserStreamState, ZapStreamDb};

#[derive(Clone)]
pub struct CfApiWrapper {
    /// Cloudflare API client
    client: CloudflareClient,
    /// Internal shared api implementation
    api_base: ApiBase,
    /// Database instance
    db: ZapStreamDb,
    /// Cache of live input data for users
    live_input_cache: Arc<RwLock<HashMap<u64, LiveInput>>>,
    /// Terms of Service URL to return in account info
    tos_url: Option<String>,
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
    ) -> Self {
        Self {
            client: CloudflareClient::new(token),
            api_base: ApiBase::new(db.clone(), client.clone(), lightning),
            db,
            live_input_cache: Default::default(),
            tos_url: None,
            create_input_lock: Default::default(),
            public_url,
            webhook_details: Default::default(),
            n53: N53Publisher::new(stream_manager.clone(), client.clone()),
            stream_manager,
        }
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

        let Some(user) = self.db.get_user_by_external_id(input_id).await? else {
            bail!("No user found with external_id {}", input_id);
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
                    // TODO: upstream stream recording
                    let input = self.get_user_live_input_by_input_id(&v.live_input).await?;
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

    async fn detect_endpoint(&self, connection: &ConnectionInfo) -> Result<IngestEndpoint> {
        // TODO: allow user to select their default endpoint

        let endpoints = self.db.get_ingest_endpoints().await?;

        if endpoints.is_empty() {
            bail!("No endpoints found, please configure endpoints first!");
        }
        let default = endpoints.iter().max_by_key(|e| e.cost);
        Ok(endpoints
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(&connection.app_name))
            .or(default)
            .unwrap()
            .clone())
    }

    async fn publish_stream_start(&self, input: LiveInput) -> Result<()> {
        let user = self.db.get_user_by_external_id(&input.uid).await?;
        let Some(user) = user else {
            bail!("No user found with external_id {}", input.uid);
        };
        let new_id = Uuid::new_v4();
        let conn = ConnectionInfo {
            id: new_id,
            endpoint: "RTMPS".to_string(),
            ip_addr: "".to_string(),
            app_name: "cloudflare".to_string(),
            key: input.rtmps.stream_key.clone(),
        };
        let endpoint = self.detect_endpoint(&conn).await?;
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

        let stream_event = self.publish_stream_event(&new_stream, &user).await?;
        new_stream.event = Some(stream_event.as_json());
        self.db.update_stream(&new_stream).await?;

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
        let Some(user) = user else {
            bail!("No user found with external_id {}", input.uid);
        };

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

    pub async fn publish_stream_event(&self, stream: &UserStream, user: &User) -> Result<Event> {
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
        let ev = self.n53.stream_to_event(stream, extra_tags).await?;
        self.n53.publish(&ev).await?;
        info!("Published stream event {}", ev.id.to_hex());
        Ok(ev)
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
        let Some(ingest_endpoint) = ingests.iter().max_by_key(|k| k.cost) else {
            bail!("No ingest endpoints found");
        };

        Ok(AccountInfo {
            endpoints: vec![Endpoint {
                name: "RTMPS".to_string(),
                url: input.rtmps.url.clone(),
                key: input.rtmps.stream_key.clone(),
                capabilities: vec![],
                cost: EndpointCost {
                    unit: "min".to_string(),
                    rate: ingest_endpoint.cost as f32 / 1000.,
                },
            }],
            balance: user.balance / 1000,
            tos: AccountTos {
                accepted: user.tos_accepted.is_some(),
                link: self
                    .tos_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or("https://zap.stream/tos")
                    .to_string(),
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

    async fn get_stream_keys(&self, _auth: Nip98Auth) -> Result<Vec<StreamKey>> {
        Ok(Vec::new())
    }

    async fn create_stream_key(
        &self,
        _auth: Nip98Auth,
        _req: CreateStreamKeyRequest,
    ) -> Result<CreateStreamKeyResponse> {
        bail!("Not supported");
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
