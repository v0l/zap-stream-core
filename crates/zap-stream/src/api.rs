use crate::http::check_nip98_auth;
use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use crate::ListenerEndpoint;
use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use log::warn;
use matchit::Router;
use nostr_sdk::{serde_json, JsonUtil, PublicKey};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;
use zap_stream_db::{UserStream, ZapStreamDb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Account,
    Topup,
    Event,
    Withdraw,
    Forward,
    ForwardId,
    History,
    Keys,
}

#[derive(Clone)]
pub struct Api {
    db: ZapStreamDb,
    settings: Settings,
    lnd: fedimint_tonic_lnd::Client,
    router: Router<Route>,
    overseer: Arc<ZapStreamOverseer>,
}

impl Api {
    pub fn new(overseer: Arc<ZapStreamOverseer>, settings: Settings) -> Self {
        let mut router = Router::new();

        // Define routes (path only, method will be matched separately)
        router.insert("/api/v1/account", Route::Account).unwrap();
        router.insert("/api/v1/topup", Route::Topup).unwrap();
        router.insert("/api/v1/event", Route::Event).unwrap();
        router.insert("/api/v1/withdraw", Route::Withdraw).unwrap();
        router.insert("/api/v1/forward", Route::Forward).unwrap();
        router
            .insert("/api/v1/forward/{id}", Route::ForwardId)
            .unwrap();
        router.insert("/api/v1/history", Route::History).unwrap();
        router.insert("/api/v1/keys", Route::Keys).unwrap();

        Self {
            db: overseer.database(),
            settings,
            lnd: overseer.lnd_client(),
            router,
            overseer,
        }
    }

    pub async fn handler(
        self,
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        let base = Response::builder()
            .header("server", "zap-stream")
            .header("content-type", "application/json")
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-headers", "*")
            .header(
                "access-control-allow-methods",
                "HEAD, GET, PATCH, DELETE, POST, OPTIONS",
            );

        // Handle OPTIONS requests
        if req.method() == Method::OPTIONS {
            return Ok(base.body(Default::default())?);
        }

        // Route matching
        let path = req.uri().path();
        let matched = self.router.at(path);

        if let Ok(matched) = matched {
            let route = *matched.value;
            let params = matched.params;

            match (req.method(), route) {
                (&Method::GET, Route::Account) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let rsp = self.get_account(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::PATCH, Route::Account) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let body = req.collect().await?.to_bytes();
                    let r_body: PatchAccount = serde_json::from_slice(&body)?;
                    let rsp = self.update_account(&auth.pubkey, r_body).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::Topup) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: Url = full_url.parse()?;
                    let amount: usize = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "amount" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .ok_or(anyhow!("Missing amount"))?;
                    let rsp = self.topup(&auth.pubkey, amount).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::PATCH, Route::Event) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let body = req.collect().await?.to_bytes();
                    let patch_event: PatchEvent = serde_json::from_slice(&body)?;
                    let rsp = self.update_event(&auth.pubkey, patch_event).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::Withdraw) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: Url = full_url.parse()?;
                    let invoice = url
                        .query_pairs()
                        .find_map(|(k, v)| {
                            if k == "invoice" {
                                Some(v.to_string())
                            } else {
                                None
                            }
                        })
                        .ok_or(anyhow!("Missing invoice parameter"))?;
                    let rsp = self.withdraw(&auth.pubkey, invoice).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::Forward) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let body = req.collect().await?.to_bytes();
                    let forward_req: ForwardRequest = serde_json::from_slice(&body)?;
                    let rsp = self.create_forward(&auth.pubkey, forward_req).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::DELETE, Route::ForwardId) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let forward_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing forward ID"))?;
                    let rsp = self.delete_forward(&auth.pubkey, forward_id).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::History) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let rsp = self.get_account_history(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::Keys) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let rsp = self.get_account_keys(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::Keys) => {
                    let auth = check_nip98_auth(&req, &self.settings.public_url)?;
                    let body = req.collect().await?.to_bytes();
                    let create_req: CreateStreamKeyRequest = serde_json::from_slice(&body)?;
                    let rsp = self.create_stream_key(&auth.pubkey, create_req).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                _ => Ok(base.status(405).body(Default::default())?), // Method not allowed
            }
        } else {
            Ok(base.status(404).body(Default::default())?) // Not found
        }
    }

    fn body_json<T: Serialize>(obj: &T) -> Result<BoxBody<Bytes, anyhow::Error>> {
        Ok(Full::from(serde_json::to_string(obj)?)
            .map_err(|e| match e {})
            .boxed())
    }

    async fn get_account(&self, pubkey: &PublicKey) -> Result<AccountInfo> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let user = self.db.get_user(uid).await?;

        // Get user forwards
        let forwards = self.db.get_user_forwards(uid).await?;

        // Get ingest endpoints from database
        let db_ingest_endpoints = self.db.get_ingest_endpoints().await?;

        // Create 2D array: settings endpoints × database ingest endpoints
        let mut endpoints = Vec::new();

        for setting_endpoint in &self.settings.endpoints {
            if let Ok(listener_endpoint) = ListenerEndpoint::from_str(&setting_endpoint) {
                match listener_endpoint {
                    ListenerEndpoint::SRT { endpoint } => {
                        if let Ok(addr) = endpoint.parse::<SocketAddr>() {
                            for ingest in &db_ingest_endpoints {
                                endpoints.push(Endpoint {
                                    name: format!("SRT-{}", ingest.name),
                                    url: format!(
                                        "srt://{}:{}",
                                        self.settings.endpoints_public_hostname,
                                        addr.port()
                                    ),
                                    key: user.stream_key.clone(),
                                    capabilities: ingest
                                        .capabilities
                                        .as_ref()
                                        .map(|c| {
                                            c.split(',').map(|s| s.trim().to_string()).collect()
                                        })
                                        .unwrap_or_else(Vec::new),
                                    cost: EndpointCost {
                                        unit: "min".to_string(),
                                        rate: ingest.cost as f32 / 1000.0,
                                    },
                                });
                            }
                        }
                    }
                    ListenerEndpoint::RTMP { endpoint } => {
                        if let Ok(addr) = endpoint.parse::<SocketAddr>() {
                            for ingest in &db_ingest_endpoints {
                                endpoints.push(Endpoint {
                                    name: format!("RTMP-{}", ingest.name),
                                    url: format!(
                                        "rtmp://{}:{}",
                                        self.settings.endpoints_public_hostname,
                                        addr.port()
                                    ),
                                    key: user.stream_key.clone(),
                                    capabilities: ingest
                                        .capabilities
                                        .as_ref()
                                        .map(|c| {
                                            c.split(',').map(|s| s.trim().to_string()).collect()
                                        })
                                        .unwrap_or_else(Vec::new),
                                    cost: EndpointCost {
                                        unit: "min".to_string(),
                                        rate: ingest.cost as f32 / 1000.0,
                                    },
                                });
                            }
                        }
                    }
                    ListenerEndpoint::TCP { endpoint } => {
                        if let Ok(addr) = endpoint.parse::<SocketAddr>() {
                            for ingest in &db_ingest_endpoints {
                                endpoints.push(Endpoint {
                                    name: format!("TCP-{}", ingest.name),
                                    url: format!(
                                        "tcp://{}:{}",
                                        self.settings.endpoints_public_hostname,
                                        addr.port()
                                    ),
                                    key: user.stream_key.clone(),
                                    capabilities: ingest
                                        .capabilities
                                        .as_ref()
                                        .map(|c| {
                                            c.split(',').map(|s| s.trim().to_string()).collect()
                                        })
                                        .unwrap_or_else(Vec::new),
                                    cost: EndpointCost {
                                        unit: "min".to_string(),
                                        rate: ingest.cost as f32 / 1000.0,
                                    },
                                });
                            }
                        }
                    }
                    ListenerEndpoint::File { .. } => {}
                    ListenerEndpoint::TestPattern => {}
                }
            }
        }

        Ok(AccountInfo {
            endpoints,
            balance: user.balance as u64,
            tos: AccountTos {
                accepted: user.tos_accepted.is_some(),
                link: "https://zap.stream/tos".to_string(),
            },
            forwards: forwards
                .into_iter()
                .map(|f| ForwardDest {
                    id: f.id,
                    name: f.name,
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
        })
    }

    async fn update_account(&self, pubkey: &PublicKey, account: PatchAccount) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        if let Some(accept_tos) = account.accept_tos {
            if accept_tos {
                let user = self.db.get_user(uid).await?;
                if user.tos_accepted.is_none() {
                    self.db.accept_tos(uid).await?;
                }
            }
        }

        Ok(())
    }

    async fn topup(&self, pubkey: &PublicKey, amount: usize) -> Result<TopupResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        // Create Lightning invoice
        let invoice_req = fedimint_tonic_lnd::lnrpc::Invoice {
            value: amount as i64,
            memo: format!(
                "zap.stream topup for user {}",
                hex::encode(pubkey.to_bytes())
            ),
            ..Default::default()
        };

        let response = self
            .lnd
            .clone()
            .lightning()
            .add_invoice(invoice_req)
            .await?;
        let invoice_response = response.into_inner();

        // Create payment entry for this topup invoice
        self.db
            .create_payment(
                &invoice_response.r_hash,
                uid,
                Some(&invoice_response.payment_request),
                amount as u64 * 1000, // Convert to milli-sats
                zap_stream_db::PaymentType::TopUp,
                0,
            )
            .await?;

        Ok(TopupResponse {
            pr: invoice_response.payment_request,
        })
    }

    async fn update_event(&self, pubkey: &PublicKey, patch_event: PatchEvent) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        if patch_event
            .id
            .as_ref()
            .map(|i| !i.is_empty())
            .unwrap_or(false)
        {
            // Update specific stream
            let stream_uuid = Uuid::parse_str(&patch_event.id.unwrap())?;
            let mut stream = self.db.get_stream(&stream_uuid).await?;

            // Verify user owns this stream
            if stream.user_id != uid {
                bail!("Unauthorized: Stream belongs to different user");
            }

            // Update stream with patch data
            if let Some(title) = patch_event.title {
                stream.title = Some(title);
            }
            if let Some(summary) = patch_event.summary {
                stream.summary = Some(summary);
            }
            if let Some(image) = patch_event.image {
                stream.image = Some(image);
            }
            if let Some(tags) = patch_event.tags {
                stream.tags = Some(tags.join(","));
            }
            if let Some(content_warning) = patch_event.content_warning {
                stream.content_warning = Some(content_warning);
            }
            if let Some(goal) = patch_event.goal {
                stream.goal = Some(goal);
            }

            self.db.update_stream(&stream).await?;

            // Update the nostr event and republish like C# version
            if let Err(e) = self
                .republish_stream_event(&stream, pubkey.to_bytes())
                .await
            {
                warn!(
                    "Failed to republish nostr event for stream {}: {}",
                    stream.id, e
                );
            }
        } else {
            // Update user default stream info
            self.db
                .update_user_defaults(
                    uid,
                    patch_event.title.as_deref(),
                    patch_event.summary.as_deref(),
                    patch_event.image.as_deref(),
                    patch_event.tags.as_ref().map(|t| t.join(",")).as_deref(),
                    patch_event.content_warning.as_deref(),
                    patch_event.goal.as_deref(),
                )
                .await?;
        }

        Ok(())
    }

    async fn withdraw(&self, pubkey: &PublicKey, invoice: String) -> Result<WithdrawResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let user = self.db.get_user(uid).await?;

        let mut lnd = self.lnd.clone();

        // Decode invoice to get amount and payment hash
        let decode_req = fedimint_tonic_lnd::lnrpc::PayReqString {
            pay_req: invoice.clone(),
        };
        let decode_response = lnd.lightning().decode_pay_req(decode_req).await?;
        let decoded = decode_response.into_inner();
        let invoice_amount = decoded.num_msat as u64;
        let payment_hash = hex::decode(decoded.payment_hash)?;

        // Check if user has sufficient balance
        if user.balance < invoice_amount as i64 {
            bail!("Insufficient balance");
        }

        // 1. Deduct balance first (safer approach)
        self.db
            .update_user_balance(uid, -(invoice_amount as i64))
            .await?;

        // 2. Create payment record
        self.db
            .create_payment(
                &payment_hash,
                uid,
                Some(&invoice),
                invoice_amount,
                zap_stream_db::PaymentType::Withdrawal,
                0,
            )
            .await?;

        // 3. Attempt Lightning payment
        let send_req = fedimint_tonic_lnd::lnrpc::SendRequest {
            payment_request: invoice.clone(),
            ..Default::default()
        };

        let response = lnd.lightning().send_payment_sync(send_req).await;

        match response {
            Ok(resp) => {
                let payment_response = resp.into_inner();
                if payment_response.payment_error.is_empty() {
                    // Payment successful
                    let fee = payment_response
                        .payment_route
                        .map(|r| r.total_fees_msat)
                        .unwrap_or(0);

                    // Update payment record with fee and mark as paid
                    self.db.complete_payment(&payment_hash, fee as u64).await?;

                    // Deduct additional fee if any
                    if fee > 0 {
                        self.db.update_user_balance(uid, -fee).await?;
                    }

                    Ok(WithdrawResponse {
                        fee,
                        preimage: hex::encode(payment_response.payment_preimage),
                    })
                } else {
                    // Payment failed, reverse balance deduction
                    self.db
                        .update_user_balance(uid, invoice_amount as i64)
                        .await?;
                    bail!("Payment failed: {}", payment_response.payment_error);
                }
            }
            Err(e) => {
                // Payment failed, reverse balance deduction
                self.db
                    .update_user_balance(uid, invoice_amount as i64)
                    .await?;
                bail!("Payment failed: {}", e);
            }
        }
    }

    async fn create_forward(
        &self,
        pubkey: &PublicKey,
        req: ForwardRequest,
    ) -> Result<ForwardResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let forward_id = self.db.create_forward(uid, &req.name, &req.target).await?;

        Ok(ForwardResponse { id: forward_id })
    }

    async fn delete_forward(&self, pubkey: &PublicKey, forward_id: &str) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let forward_id: u64 = forward_id.parse()?;
        self.db.delete_forward(uid, forward_id).await?;
        Ok(())
    }

    async fn get_account_history(&self, pubkey: &PublicKey) -> Result<HistoryResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        // For now, just get first page with default page size
        let payments = self.db.get_payment_history(uid, 0, 100).await?;

        let items = payments
            .into_iter()
            .filter(|p| p.is_paid) // Only include paid payments like C# version
            .map(|p| HistoryEntry {
                created: p.created.timestamp() as u64,
                entry_type: match p.payment_type {
                    zap_stream_db::PaymentType::Withdrawal => 1, // Debit
                    _ => 0, // Credit (TopUp, Zap, Credit, AdmissionFee)
                },
                amount: p.amount as f64 / 1000.0, // Convert from milli-sats to sats
                desc: match p.payment_type {
                    zap_stream_db::PaymentType::Withdrawal => Some("Withdrawal".to_string()),
                    zap_stream_db::PaymentType::Credit => Some("Admin Credit".to_string()),
                    zap_stream_db::PaymentType::Zap => p.nostr.clone(), // Nostr content
                    _ => None,
                },
            })
            .collect();

        // TODO: past streams should include a history entry

        Ok(HistoryResponse {
            items,
            page: 0,
            page_size: 100,
        })
    }

    async fn get_account_keys(&self, pubkey: &PublicKey) -> Result<Vec<StreamKey>> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let keys = self.db.get_user_stream_keys(uid).await?;

        Ok(keys
            .into_iter()
            .map(|k| StreamKey {
                id: k.id,
                key: k.key,
                created: k.created.timestamp(),
                expires: k.expires.map(|e| e.timestamp()),
                stream_id: k.stream_id,
            })
            .collect())
    }

    async fn create_stream_key(
        &self,
        pubkey: &PublicKey,
        req: CreateStreamKeyRequest,
    ) -> Result<CreateStreamKeyResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        // Create a new stream record for this key
        let stream_id = Uuid::new_v4();
        let new_stream = zap_stream_db::UserStream {
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

        // Create the stream record
        self.db.insert_stream(&new_stream).await?;

        // Generate a new stream key
        let key = Uuid::new_v4().to_string();
        let _key_id = self
            .db
            .create_stream_key(uid, &key, req.expires, &stream_id.to_string())
            .await?;

        // For now, return minimal response - event building would require nostr integration
        Ok(CreateStreamKeyResponse {
            key,
            event: None, // TODO: Build proper nostr event like C# version
        })
    }

    /// Republish stream event to nostr relays using the same code as overseer
    async fn republish_stream_event(&self, stream: &UserStream, pubkey: [u8; 32]) -> Result<()> {
        let event = self
            .overseer
            .publish_stream_event(stream, &pubkey.to_vec())
            .await?;

        // Update the stream with the new event JSON
        let mut updated_stream = stream.clone();
        updated_stream.event = Some(event.as_json());
        self.db.update_stream(&updated_stream).await?;

        Ok(())
    }

    /// Track a viewer for viewer count analytics
    pub fn track_viewer(&self, token: &str, stream_id: &str, ip_address: &str, user_agent: Option<String>) {
        self.overseer.viewer_tracker().track_viewer(token, stream_id, ip_address, user_agent);
    }

    /// Get current viewer count for a stream
    pub fn get_viewer_count(&self, stream_id: &str) -> usize {
        self.overseer.viewer_tracker().get_viewer_count(stream_id)
    }

    /// Get active streams from database
    pub async fn get_active_streams(&self) -> Result<Vec<UserStream>> {
        self.db.list_live_streams().await
    }

    /// Get the public URL from settings
    pub fn get_public_url(&self) -> String {
        self.settings.public_url.clone()
    }
}

#[derive(Deserialize, Serialize)]
struct AccountInfo {
    pub endpoints: Vec<Endpoint>,
    pub balance: u64,
    pub tos: AccountTos,
    pub forwards: Vec<ForwardDest>,
    pub details: Option<PatchEventDetails>,
}

#[derive(Deserialize, Serialize)]
struct Endpoint {
    pub name: String,
    pub url: String,
    pub key: String,
    pub capabilities: Vec<String>,
    pub cost: EndpointCost,
}

#[derive(Deserialize, Serialize)]
struct EndpointCost {
    pub unit: String,
    pub rate: f32,
}

#[derive(Deserialize, Serialize)]
struct AccountTos {
    pub accepted: bool,
    pub link: String,
}

#[derive(Deserialize, Serialize)]
struct PatchAccount {
    pub accept_tos: Option<bool>,
}

#[derive(Deserialize, Serialize)]
struct TopupResponse {
    pub pr: String,
}

#[derive(Deserialize, Serialize)]
struct WithdrawRequest {
    pub payment_request: String,
    pub amount: u64,
}

#[derive(Deserialize, Serialize)]
struct WithdrawResponse {
    pub fee: i64,
    pub preimage: String,
}

#[derive(Deserialize, Serialize)]
struct ForwardRequest {
    pub name: String,
    pub target: String,
}

#[derive(Deserialize, Serialize)]
struct ForwardResponse {
    pub id: u64,
}

#[derive(Deserialize, Serialize)]
struct HistoryEntry {
    pub created: u64,
    #[serde(rename = "type")]
    pub entry_type: i32,
    pub amount: f64,
    pub desc: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct HistoryResponse {
    pub items: Vec<HistoryEntry>,
    pub page: i32,
    pub page_size: i32,
}

#[derive(Deserialize, Serialize)]
struct StreamKey {
    pub id: u64,
    pub key: String,
    pub created: i64,
    pub expires: Option<i64>,
    pub stream_id: String,
}

#[derive(Deserialize, Serialize)]
struct CreateStreamKeyRequest {
    pub event: PatchEventDetails,
    pub expires: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize)]
struct CreateStreamKeyResponse {
    pub key: String,
    pub event: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct PatchEvent {
    pub id: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub tags: Option<Vec<String>>,
    pub content_warning: Option<String>,
    pub goal: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct PatchEventDetails {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub tags: Option<Vec<String>>,
    pub content_warning: Option<String>,
    pub goal: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct ForwardDest {
    pub id: u64,
    pub name: String,
}
