use crate::http::{check_nip98_auth, HttpFuture, HttpServerPlugin, StreamData};
use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use crate::stream_manager::StreamManager;
use crate::websocket_metrics::WebSocketMetricsServer;
use crate::ListenerEndpoint;
use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use log::{error, info, warn};
use matchit::Router;
use nostr_sdk::prelude::EventDeletionRequest;
use nostr_sdk::{serde_json, Client, PublicKey};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::overseer::Overseer;
use zap_stream_db::ZapStreamDb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Account,
    #[cfg(feature = "zap-stream")]
    Topup,
    Event,
    #[cfg(feature = "zap-stream")]
    Withdraw,
    Forward,
    ForwardId,
    History,
    Keys,
    AdminUsers,
    AdminUsersId,
    AdminUserHistory,
    AdminUserStreams,
    DeleteStream,
}

#[derive(Clone)]
pub struct Api {
    db: ZapStreamDb,
    settings: Settings,
    #[cfg(feature = "zap-stream")]
    lnd: fedimint_tonic_lnd::Client,
    router: Router<Route>,
    overseer: Arc<dyn Overseer>,
    stream_manager: StreamManager,
    nostr_client: Client,
}

impl Api {
    pub fn new(overseer: Arc<ZapStreamOverseer>, settings: Settings) -> Self {
        let mut router = Router::new();

        // Define routes (path only, method will be matched separately)
        router.insert("/api/v1/account", Route::Account).unwrap();
        #[cfg(feature = "zap-stream")]
        router.insert("/api/v1/topup", Route::Topup).unwrap();
        router.insert("/api/v1/event", Route::Event).unwrap();
        #[cfg(feature = "zap-stream")]
        router.insert("/api/v1/withdraw", Route::Withdraw).unwrap();
        router.insert("/api/v1/forward", Route::Forward).unwrap();
        router
            .insert("/api/v1/forward/{id}", Route::ForwardId)
            .unwrap();
        router.insert("/api/v1/history", Route::History).unwrap();
        router.insert("/api/v1/keys", Route::Keys).unwrap();
        router
            .insert("/api/v1/admin/users", Route::AdminUsers)
            .unwrap();
        router
            .insert("/api/v1/admin/users/{id}", Route::AdminUsersId)
            .unwrap();
        router
            .insert("/api/v1/admin/users/{id}/history", Route::AdminUserHistory)
            .unwrap();
        router
            .insert("/api/v1/admin/users/{id}/streams", Route::AdminUserStreams)
            .unwrap();
        router
            .insert("/api/v1/stream/{id}", Route::DeleteStream)
            .unwrap();

        Self {
            db: overseer.database(),
            settings,
            #[cfg(feature = "zap-stream")]
            lnd: overseer.lnd_client(),
            router,
            stream_manager: overseer.stream_manager(),
            nostr_client: overseer.nostr_client(),
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

        // Authenticate all API requests
        let auth = check_nip98_auth(&req, &self.settings.public_url, &self.db).await?;

        // Route matching
        let path = req.uri().path().to_string();
        let method = req.method().clone();
        let matched = self.router.at(&path);

        if let Ok(matched) = matched {
            let route = *matched.value;
            let params = matched.params;

            match (&method, route) {
                (&Method::GET, Route::Account) => {
                    let rsp = self.get_account(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::PATCH, Route::Account) => {
                    let body = req.collect().await?.to_bytes();
                    let r_body: PatchAccount = serde_json::from_slice(&body)?;
                    self.update_account(&auth.pubkey, r_body).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                #[cfg(feature = "zap-stream")]
                (&Method::GET, Route::Topup) => {
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: url::Url = full_url.parse()?;
                    let amount: usize = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "amount" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .ok_or(anyhow!("Missing amount"))?;
                    let rsp = self.topup(&auth.pubkey, amount).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::PATCH, Route::Event) => {
                    let body = req.collect().await?.to_bytes();
                    let patch_event: PatchEvent = serde_json::from_slice(&body)?;
                    self.update_event(&auth.pubkey, patch_event).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                #[cfg(feature = "zap-stream")]
                (&Method::POST, Route::Withdraw) => {
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: url::Url = full_url.parse()?;
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
                    let body = req.collect().await?.to_bytes();
                    let forward_req: ForwardRequest = serde_json::from_slice(&body)?;
                    let rsp = self.create_forward(&auth.pubkey, forward_req).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::DELETE, Route::ForwardId) => {
                    let forward_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing forward ID"))?;
                    self.delete_forward(&auth.pubkey, forward_id).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::GET, Route::History) => {
                    let rsp = self.get_account_history(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::Keys) => {
                    let rsp = self.get_account_keys(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::Keys) => {
                    let body = req.collect().await?.to_bytes();
                    let create_req: CreateStreamKeyRequest = serde_json::from_slice(&body)?;
                    let rsp = self.create_stream_key(&auth.pubkey, create_req).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::AdminUsers) => {
                    self.check_admin_access(&auth.pubkey).await?;
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: url::Url = full_url.parse()?;
                    let page: u64 = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "page" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let limit: u64 = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "limit" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(50);
                    let search = url.query_pairs().find_map(|(k, v)| {
                        if k == "search" {
                            Some(v.to_string())
                        } else {
                            None
                        }
                    });
                    let rsp = self.admin_list_users(page, limit, search).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::AdminUsersId) => {
                    self.check_admin_access(&auth.pubkey).await?;
                    let user_id = params.get("id").ok_or_else(|| anyhow!("Missing user ID"))?;
                    let body = req.collect().await?.to_bytes();
                    let admin_req: AdminUserRequest = serde_json::from_slice(&body)?;
                    self.admin_manage_user(user_id, admin_req).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::GET, Route::AdminUserHistory) => {
                    self.check_admin_access(&auth.pubkey).await?;
                    let user_id = params.get("id").ok_or_else(|| anyhow!("Missing user ID"))?;
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: url::Url = full_url.parse()?;
                    let page: u64 = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "page" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let limit: u64 = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "limit" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(50);
                    let uid: u64 = user_id.parse()?;
                    let rsp = self.get_user_history(uid, page, limit).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::AdminUserStreams) => {
                    self.check_admin_access(&auth.pubkey).await?;
                    let user_id = params.get("id").ok_or_else(|| anyhow!("Missing user ID"))?;
                    let full_url = format!(
                        "{}{}",
                        self.settings.public_url.trim_end_matches('/'),
                        req.uri()
                    );
                    let url: url::Url = full_url.parse()?;
                    let page: u64 = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "page" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let limit: u64 = url
                        .query_pairs()
                        .find_map(|(k, v)| if k == "limit" { Some(v) } else { None })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(50);
                    let uid: u64 = user_id.parse()?;
                    let rsp = self.admin_get_user_streams(uid, page, limit).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::DELETE, Route::DeleteStream) => {
                    let stream_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing stream ID"))?;
                    self.delete_stream(&auth.pubkey, stream_id).await?;
                    Ok(base.body(Self::body_json(&())?)?)
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
            if let Ok(listener_endpoint) = ListenerEndpoint::from_str(setting_endpoint) {
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

    #[cfg(feature = "zap-stream")]
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

            // Don't allow modifications of ended streams
            if stream.state == zap_stream_db::UserStreamState::Ended {
                bail!("Cannot modify ended stream");
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
            if let Err(e) = self.overseer.on_update(&stream_uuid).await {
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

    #[cfg(feature = "zap-stream")]
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

                    // Update payment record with fee and mark as paid (for withdrawals - subtracts fee)
                    self.db
                        .complete_withdrawal(&payment_hash, fee as u64)
                        .await?;

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
        self.get_user_history(uid, 0, 100).await
    }

    async fn get_user_history(&self, uid: u64, page: u64, limit: u64) -> Result<HistoryResponse> {
        let offset = page * limit;
        let payments = self.db.get_payment_history(uid, offset, limit).await?;

        let mut items: Vec<HistoryEntry> = payments
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

        // Add ended streams as debit entries
        let ended_streams = self.db.get_user_ended_streams(uid).await?;
        let stream_entries: Vec<HistoryEntry> = ended_streams
            .into_iter()
            .map(|s| HistoryEntry {
                created: s.ends.unwrap_or(s.starts).timestamp() as u64, // Use end time, fallback to start time
                entry_type: 1,                                          // Debit
                amount: s.cost as f64 / 1000.0, // Convert from milli-sats to sats
                desc: Some(format!(
                    "Stream: {}",
                    s.title.unwrap_or_else(|| s.id.clone())
                )),
            })
            .collect();

        items.extend(stream_entries);

        // Sort all items by created time (descending)
        items.sort_by(|a, b| b.created.cmp(&a.created));

        Ok(HistoryResponse {
            items,
            page: page as i32,
            page_size: limit as i32,
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

    async fn check_admin_access(&self, pubkey: &PublicKey) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let is_admin = self.db.is_admin(uid).await?;
        if !is_admin {
            bail!("Access denied: Admin privileges required");
        }
        Ok(())
    }

    async fn delete_stream(&self, pubkey: &PublicKey, stream_id: &str) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let stream_uuid = Uuid::parse_str(stream_id)?;
        let stream = self.db.get_stream(&stream_uuid).await?;

        // Verify the user owns this stream
        if stream.user_id != uid {
            bail!("Access denied: You can only delete your own streams");
        }

        // Publish Nostr deletion request event if the stream has an associated event
        if let Some(event_json) = &stream.event {
            if let Ok(stream_event) = serde_json::from_str::<nostr_sdk::Event>(event_json) {
                let deletion_event = nostr_sdk::EventBuilder::delete(
                    EventDeletionRequest::new()
                        .id(stream_event.id)
                        .coordinate(stream_event.coordinate().unwrap().into_owned()),
                );

                if let Err(e) = self.nostr_client.send_event_builder(deletion_event).await {
                    warn!(
                        "Failed to publish deletion event for stream {}: {}",
                        stream_id, e
                    );
                } else {
                    info!("Published deletion request event for stream {}", stream_id);
                }
            }
        }

        Ok(())
    }

    async fn admin_list_users(
        &self,
        page: u64,
        limit: u64,
        search: Option<String>,
    ) -> Result<AdminUsersResponse> {
        let offset = page * limit;

        let (users, total) = if let Some(search_term) = search {
            self.db.search_users_by_pubkey(&search_term).await?
        } else {
            self.db.list_users(offset, limit).await?
        };

        let users_info: Vec<AdminUserInfo> = users
            .into_iter()
            .map(|user| AdminUserInfo {
                id: user.id,
                pubkey: hex::encode(user.pubkey),
                created: user.created.timestamp() as u64,
                balance: user.balance,
                is_admin: user.is_admin,
                is_blocked: user.is_blocked,
                tos_accepted: user.tos_accepted.map(|t| t.timestamp() as u64),
                title: user.title,
                summary: user.summary,
            })
            .collect();

        Ok(AdminUsersResponse {
            users: users_info,
            page: page as u32,
            limit: limit as u32,
            total: total as u32,
        })
    }

    async fn admin_manage_user(&self, user_id: &str, req: AdminUserRequest) -> Result<()> {
        let uid: u64 = user_id.parse()?;

        if let Some(is_admin) = req.set_admin {
            self.db.set_admin(uid, is_admin).await?;
        }

        if let Some(is_blocked) = req.set_blocked {
            self.db.set_blocked(uid, is_blocked).await?;
        }

        if let Some(credit_amount) = req.add_credit {
            if credit_amount > 0 {
                self.db
                    .add_admin_credit(uid, credit_amount, req.memo.as_deref())
                    .await?;
            }
        }

        // Update user default stream details if any are provided
        if req.title.is_some()
            || req.summary.is_some()
            || req.image.is_some()
            || req.tags.is_some()
            || req.content_warning.is_some()
            || req.goal.is_some()
        {
            self.db
                .update_user_defaults(
                    uid,
                    req.title.as_deref(),
                    req.summary.as_deref(),
                    req.image.as_deref(),
                    req.tags.as_ref().map(|tags| tags.join(",")).as_deref(),
                    req.content_warning.as_deref(),
                    req.goal.as_deref(),
                )
                .await?;
        }

        Ok(())
    }

    async fn admin_get_user_streams(
        &self,
        user_id: u64,
        page: u64,
        limit: u64,
    ) -> Result<AdminUserStreamsResponse> {
        let offset = page * limit;
        let (streams, total) = self.db.get_user_streams(user_id, offset, limit).await?;

        let streams_info: Vec<AdminStreamInfo> = streams
            .into_iter()
            .map(|stream| AdminStreamInfo {
                id: stream.id,
                starts: stream.starts.timestamp() as u64,
                ends: stream.ends.map(|e| e.timestamp() as u64),
                state: stream.state.to_string(),
                title: stream.title,
                summary: stream.summary,
                image: stream.image,
                thumb: stream.thumb,
                tags: stream
                    .tags
                    .map(|t| t.split(',').map(|s| s.trim().to_string()).collect()),
                content_warning: stream.content_warning,
                goal: stream.goal,
                cost: stream.cost,
                duration: stream.duration,
                fee: stream.fee,
                endpoint_id: stream.endpoint_id,
            })
            .collect();

        Ok(AdminUserStreamsResponse {
            streams: streams_info,
            page: page as u32,
            limit: limit as u32,
            total: total as u32,
        })
    }
}

impl HttpServerPlugin for Api {
    fn get_active_streams(&self) -> Pin<Box<dyn Future<Output = Result<Vec<StreamData>>> + Send>> {
        let db = self.db.clone();
        let viewers = self.stream_manager.clone();
        Box::pin(async move {
            let streams = db.list_live_streams().await?;
            let mut ret = Vec::with_capacity(streams.len());
            for stream in streams {
                let viewers = viewers.get_viewer_count(&stream.id).await;
                ret.push(StreamData {
                    live_url: format!("{}/{}/live.m3u8", stream.id, HlsEgress::PATH),
                    id: stream.id,
                    title: stream.title.unwrap_or_default(),
                    summary: stream.summary,
                    viewer_count: Some(viewers as _),
                });
            }
            Ok(ret)
        })
    }

    fn track_viewer(
        &self,
        stream_id: &str,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        let mgr = self.stream_manager.clone();
        let stream_id = stream_id.to_string();
        let token = token.to_string();
        Box::pin(async move {
            mgr.track_viewer(&stream_id, &token).await;
            Ok(())
        })
    }

    fn handler(self, request: Request<Incoming>) -> HttpFuture {
        Box::pin(async move { self.handler(request).await })
    }

    fn handle_websocket_metrics(self, request: Request<Incoming>) -> HttpFuture {
        let ws_server = WebSocketMetricsServer::new(
            self.db.clone(),
            self.stream_manager.clone(),
            self.settings.public_url.clone(),
        );
        Box::pin(async move {
            // Handle the WebSocket upgrade
            match ws_server.handle_websocket_upgrade(request) {
                Ok(response) => Ok(response),
                Err(e) => {
                    let msg = format!("WebSocket error: {}", e);
                    error!("{}", msg);
                    let error_response = Response::builder().status(500).body(
                        Full::new(bytes::Bytes::from(msg))
                            .map_err(|e| match e {})
                            .boxed(),
                    )?;
                    Ok(error_response)
                }
            }
        })
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

#[derive(Deserialize, Serialize)]
struct AdminUserInfo {
    pub id: u64,
    pub pubkey: String,
    pub created: u64,
    pub balance: i64,
    pub is_admin: bool,
    pub is_blocked: bool,
    pub tos_accepted: Option<u64>,
    pub title: Option<String>,
    pub summary: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct AdminUsersResponse {
    pub users: Vec<AdminUserInfo>,
    pub page: u32,
    pub limit: u32,
    pub total: u32,
}

#[derive(Deserialize, Serialize)]
struct AdminUserRequest {
    pub set_admin: Option<bool>,
    pub set_blocked: Option<bool>,
    pub add_credit: Option<u64>,
    pub memo: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub tags: Option<Vec<String>>,
    pub content_warning: Option<String>,
    pub goal: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct AdminStreamInfo {
    pub id: String,
    pub starts: u64,
    pub ends: Option<u64>,
    pub state: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub thumb: Option<String>,
    pub tags: Option<Vec<String>>,
    pub content_warning: Option<String>,
    pub goal: Option<String>,
    pub cost: u64,
    pub duration: f32,
    pub fee: Option<u32>,
    pub endpoint_id: Option<u64>,
}

#[derive(Deserialize, Serialize)]
struct AdminUserStreamsResponse {
    pub streams: Vec<AdminStreamInfo>,
    pub page: u32,
    pub limit: u32,
    pub total: u32,
}
