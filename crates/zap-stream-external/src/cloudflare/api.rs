use crate::cloudflare::{CloudflareClient, CloudflareToken, LiveInput, WebhookPayload};
use anyhow::{Result, bail};
use async_trait::async_trait;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use nostr_sdk::{Client, PublicKey, ToBech32};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::log::error;
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;
use zap_stream::api_base::ApiBase;
use zap_stream::payments::LightningNode;
use zap_stream_api_common::*;
use zap_stream_db::{User, ZapStreamDb};

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
}

impl CfApiWrapper {
    pub fn new(
        token: CloudflareToken,
        db: ZapStreamDb,
        client: Client,
        lightning: Arc<dyn LightningNode>,
    ) -> Self {
        Self {
            client: CloudflareClient::new(token),
            api_base: ApiBase::new(db.clone(), client, lightning),
            db,
            live_input_cache: Default::default(),
            tos_url: None,
        }
    }

    async fn create_user_live_input(&self, user: &User) -> Result<LiveInput> {
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

    async fn get_user_live_input(&self, user: &User) -> Result<LiveInput> {
        let cache = self.live_input_cache.read().await;
        if let Some(input) = cache.get(&user.id) {
            return Ok(input.clone());
        }
        drop(cache);

        // try to load from API next
        // to maintain compat with the backend PR fallback to the stream key if not set
        let external_id = user.external_id.as_ref().unwrap_or(&user.stream_key);
        if let Ok(response) = self.client.get_live_input(external_id).await {
            if response.success {
                self.live_input_cache
                    .write()
                    .await
                    .insert(user.id, response.result.clone());
                return Ok(response.result);
            }
        }

        warn!("Creating input for non-new user {}", user.id);
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

    /// Create a router to handle api requests internally
    pub fn make_router(&self) -> Router {
        Router::new()
            .route(
                "/api/v1/webhook/cloudflare",
                post(
                    async move |State(this): State<Self>, Json(payload): Json<WebhookPayload>| {
                        if let Err(e) = Self::handle_webhook(this, payload).await {
                            error!("Error handling webhook: {}", e);
                        }
                    },
                ),
            )
            .with_state(self.clone())
    }

    async fn handle_webhook(this: Self, payload: WebhookPayload) -> Result<()> {
        info!("Got webhook payload: {:?}", payload);
        match payload {
            WebhookPayload::LiveInput(i) => {
                info!(
                    "Received Cloudflare webhook event: {} for input_id: {}",
                    i.data.event_type, i.data.input_id
                );

                // Map Cloudflare event types to our generic events
                match i.data.event_type.as_str() {
                    "live_input.connected" => {
                        // TODO: publish stream started
                    }
                    "live_input.disconnected" | "live_input.errored" => {
                        // TODO: publish stream ended
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
                    let input = this.get_user_live_input_by_input_id(&v.live_input).await?;
                } else {
                    info!(
                        "Video Asset not ready yet (state: {}), ignoring",
                        v.status.state
                    );
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ZapStreamApi for CfApiWrapper {
    async fn get_account(&self, auth: Nip98Auth) -> Result<AccountInfo> {
        let (uid, is_new) = self.db.upsert_user_opt(&auth.pubkey).await?;
        let user = self.db.get_user(uid).await?;
        let input = if is_new {
            info!("Creating input for new user: {}", uid);
            self.create_user_live_input(&user).await?
        } else {
            self.get_user_live_input(&user).await?
        };

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
