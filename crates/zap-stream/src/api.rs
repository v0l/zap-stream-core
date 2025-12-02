use crate::http::{HttpFuture, HttpServerPlugin, StreamData, check_nip98_auth};
use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use crate::stream_manager::StreamManager;
use crate::websocket_metrics::WebSocketMetricsServer;
use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use lnurl::pay::{LnURLPayInvoice, PayResponse};
use matchit::Router;
use nostr_sdk::prelude::EventDeletionRequest;
use nostr_sdk::{Client, NostrSigner, PublicKey, serde_json};
use nwc::NWC;
use nwc::prelude::NostrWalletConnectURI;
use payments_rs::lightning::{AddInvoiceRequest, LightningNode};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::listen::ListenerEndpoint;
use zap_stream_core::overseer::Overseer;
use zap_stream_db::ZapStreamDb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Account,
    Topup,
    Withdraw,
    Zap,
    ZapCallback,
    Event,
    Forward,
    ForwardId,
    History,
    Keys,
    Time,
    AdminUsers,
    AdminUsersId,
    AdminUserHistory,
    AdminUserStreams,
    AdminUserStreamKey,
    AdminUserStreamKeyRegen,
    AdminAuditLog,
    AdminIngestEndpoints,
    AdminIngestEndpointsId,
    AdminPipelineLog,
    DeleteStream,
    WebhookBitvora,
}

#[derive(Clone)]
pub struct Api {
    db: ZapStreamDb,
    settings: Settings,
    payments: Arc<dyn LightningNode>,
    router: Router<Route>,
    overseer: Arc<ZapStreamOverseer>,
    stream_manager: StreamManager,
    nostr_client: Client,
}

impl Api {
    fn generate_endpoint_urls(&self, ingest_name: &str) -> Vec<String> {
        self.settings
            .endpoints
            .iter()
            .filter_map(|endpoint_url| {
                ListenerEndpoint::from_str(endpoint_url)
                    .ok()
                    .and_then(|endpoint| {
                        endpoint
                            .to_public_url(&self.settings.endpoints_public_hostname, ingest_name)
                    })
            })
            .collect()
    }

    pub fn new(overseer: Arc<ZapStreamOverseer>, settings: Settings) -> Self {
        let mut router = Router::new();

        // Define routes (path only, method will be matched separately)
        router.insert("/api/v1/account", Route::Account).unwrap();
        router.insert("/api/v1/event", Route::Event).unwrap();
        router.insert("/api/v1/forward", Route::Forward).unwrap();
        router
            .insert("/api/v1/forward/{id}", Route::ForwardId)
            .unwrap();
        router.insert("/api/v1/history", Route::History).unwrap();
        router.insert("/api/v1/keys", Route::Keys).unwrap();
        router.insert("/api/v1/time", Route::Time).unwrap();
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
            .insert(
                "/api/v1/admin/users/{id}/stream-key",
                Route::AdminUserStreamKey,
            )
            .unwrap();
        router
            .insert(
                "/api/v1/admin/users/{id}/stream-key/regenerate",
                Route::AdminUserStreamKeyRegen,
            )
            .unwrap();
        router
            .insert("/api/v1/admin/audit-log", Route::AdminAuditLog)
            .unwrap();
        router
            .insert(
                "/api/v1/admin/ingest-endpoints",
                Route::AdminIngestEndpoints,
            )
            .unwrap();
        router
            .insert(
                "/api/v1/admin/ingest-endpoints/{id}",
                Route::AdminIngestEndpointsId,
            )
            .unwrap();
        router
            .insert(
                "/api/v1/admin/pipeline-log/{stream_id}",
                Route::AdminPipelineLog,
            )
            .unwrap();
        router
            .insert("/api/v1/stream/{id}", Route::DeleteStream)
            .unwrap();

        router.insert("/api/v1/topup", Route::Topup).unwrap();
        #[cfg(feature = "withdrawal")]
        router.insert("/api/v1/withdraw", Route::Withdraw).unwrap();
        router
            .insert("/.well-known/lnurlp/{name}", Route::Zap)
            .unwrap();
        router
            .insert("/api/v1/zap/{pubkey}", Route::ZapCallback)
            .unwrap();
        router
            .insert("/api/v1/webhook/bitvora", Route::WebhookBitvora)
            .unwrap();

        Self {
            db: overseer.database(),
            settings,
            payments: overseer.lightning(),
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

        // Route matching
        let path = req.uri().path().to_string();
        let method = req.method().clone();
        let matched = self.router.at(&path);

        if let Ok(matched) = matched {
            let route = *matched.value;
            let params = matched.params;

            match (&method, route) {
                (&Method::GET, Route::Account) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let rsp = self.get_account(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::PATCH, Route::Account) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let body = req.collect().await?.to_bytes();
                    let r_body: PatchAccount = serde_json::from_slice(&body)?;
                    self.update_account(&auth.pubkey, r_body).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::PATCH, Route::Event) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let body = req.collect().await?.to_bytes();
                    let patch_event: PatchEvent = serde_json::from_slice(&body)?;
                    self.update_event(&auth.pubkey, patch_event).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::GET, Route::Topup) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
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
                    let rsp = self.topup(&auth.pubkey, amount * 1000, None).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                #[cfg(all(feature = "withdrawal"))]
                (&Method::POST, Route::Withdraw) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
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
                (&Method::GET, Route::Zap) => {
                    let target = params.get("name").ok_or(anyhow!("Missing name/pubkey"))?;
                    let target_user = if let Ok(pk) = hex::decode(target) {
                        self.db
                            .get_user_by_pubkey(&pk.as_slice().try_into()?)
                            .await?
                    } else {
                        None
                    };

                    if target_user.is_none() {
                        return Ok(base.status(404).body(Default::default())?);
                    }

                    let meta = vec![vec![
                        "text/plain".to_string(),
                        format!("Zap for {}", target),
                    ]];
                    let pubkey = self.nostr_client.signer().await?.get_public_key().await?;
                    let full_url = format!(
                        "{}/api/v1/zap/{}",
                        self.settings.public_url.trim_end_matches('/'),
                        target
                    );
                    let rsp = PayResponse {
                        callback: full_url,
                        max_sendable: 1_000_000_000,
                        min_sendable: 1_000,
                        tag: lnurl::Tag::PayRequest,
                        metadata: serde_json::to_string(&meta)?,
                        comment_allowed: None,
                        allows_nostr: Some(true),
                        nostr_pubkey: Some(pubkey.xonly()?),
                    };
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::ZapCallback) => {
                    let target = params.get("pubkey").ok_or(anyhow!("Missing name/pubkey"))?;
                    let target_user = if let Ok(pk) = hex::decode(target) {
                        self.db
                            .get_user_by_pubkey(&pk.as_slice().try_into()?)
                            .await?
                    } else {
                        None
                    };
                    let target_user = if let Some(tu) = target_user {
                        tu
                    } else {
                        return Ok(base.status(404).body(Default::default())?);
                    };

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
                    let zap_request: Option<String> = url.query_pairs().find_map(|(k, v)| {
                        if k == "nostr" {
                            Some(v.to_string())
                        } else {
                            None
                        }
                    });

                    let user_pubkey = PublicKey::from_slice(&target_user.pubkey)?;
                    let topup = self.topup(&user_pubkey, amount, zap_request).await?;
                    let rsp = LnURLPayInvoice::new(topup.pr);
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::Forward) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let body = req.collect().await?.to_bytes();
                    let forward_req: ForwardRequest = serde_json::from_slice(&body)?;
                    let rsp = self.create_forward(&auth.pubkey, forward_req).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::DELETE, Route::ForwardId) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let forward_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing forward ID"))?;
                    self.delete_forward(&auth.pubkey, forward_id).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::GET, Route::History) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let rsp = self.get_account_history(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::Keys) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let rsp = self.get_account_keys(&auth.pubkey).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::Keys) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let body = req.collect().await?.to_bytes();
                    let create_req: CreateStreamKeyRequest = serde_json::from_slice(&body)?;
                    let rsp = self.create_stream_key(&auth.pubkey, create_req).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::Time) => {
                    let time_ms = Utc::now().timestamp_millis() as u64;
                    let rsp = TimeResponse { time: time_ms };
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::AdminUsers) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let _admin_uid = self.check_admin_access(&auth.pubkey).await?;
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
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let user_id = params.get("id").ok_or_else(|| anyhow!("Missing user ID"))?;
                    let body = req.collect().await?.to_bytes();
                    let admin_req: AdminUserRequest = serde_json::from_slice(&body)?;
                    self.admin_manage_user(admin_uid, user_id, admin_req)
                        .await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::GET, Route::AdminUserHistory) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let _admin_uid = self.check_admin_access(&auth.pubkey).await?;
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
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let _admin_uid = self.check_admin_access(&auth.pubkey).await?;
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
                (&Method::GET, Route::AdminUserStreamKey) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let user_id = params.get("id").ok_or_else(|| anyhow!("Missing user ID"))?;
                    let uid: u64 = user_id.parse()?;
                    let rsp = self.admin_get_user_stream_key(uid, admin_uid).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::AdminUserStreamKeyRegen) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let user_id = params.get("id").ok_or_else(|| anyhow!("Missing user ID"))?;
                    let uid: u64 = user_id.parse()?;
                    let rsp = self
                        .admin_regenerate_user_stream_key(admin_uid, uid)
                        .await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::AdminAuditLog) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let _admin_uid = self.check_admin_access(&auth.pubkey).await?;
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
                    let rsp = self.admin_get_audit_logs(page, limit).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::AdminIngestEndpoints) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
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
                    let rsp = self
                        .admin_list_ingest_endpoints(admin_uid, page, limit)
                        .await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::POST, Route::AdminIngestEndpoints) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let body = req.collect().await?.to_bytes();
                    let endpoint_req: AdminIngestEndpointRequest = serde_json::from_slice(&body)?;
                    let rsp = self
                        .admin_create_ingest_endpoint(admin_uid, endpoint_req)
                        .await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::GET, Route::AdminIngestEndpointsId) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let endpoint_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing endpoint ID"))?;
                    let id: u64 = endpoint_id.parse()?;
                    let rsp = self.admin_get_ingest_endpoint(admin_uid, id).await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::PATCH, Route::AdminIngestEndpointsId) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let endpoint_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing endpoint ID"))?;
                    let id: u64 = endpoint_id.parse()?;
                    let body = req.collect().await?.to_bytes();
                    let endpoint_req: AdminIngestEndpointRequest = serde_json::from_slice(&body)?;
                    let rsp = self
                        .admin_update_ingest_endpoint(admin_uid, id, endpoint_req)
                        .await?;
                    Ok(base.body(Self::body_json(&rsp)?)?)
                }
                (&Method::DELETE, Route::AdminIngestEndpointsId) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let endpoint_id = params
                        .get("id")
                        .ok_or_else(|| anyhow!("Missing endpoint ID"))?;
                    let id: u64 = endpoint_id.parse()?;
                    self.admin_delete_ingest_endpoint(admin_uid, id).await?;
                    Ok(base.body(Self::body_json(&())?)?)
                }
                (&Method::GET, Route::AdminPipelineLog) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
                    let admin_uid = self.check_admin_access(&auth.pubkey).await?;
                    let stream_id = params
                        .get("stream_id")
                        .ok_or_else(|| anyhow!("Missing stream_id"))?;
                    let log_content = self.admin_get_pipeline_log(admin_uid, stream_id).await?;
                    let response = Response::builder()
                        .header("server", "zap-stream")
                        .header("content-type", "text/plain; charset=utf-8")
                        .header("access-control-allow-origin", "*")
                        .header("access-control-allow-headers", "*")
                        .header(
                            "access-control-allow-methods",
                            "HEAD, GET, PATCH, DELETE, POST, OPTIONS",
                        )
                        .body(Full::from(log_content).map_err(|e| match e {}).boxed())?;
                    Ok(response)
                }
                (&Method::DELETE, Route::DeleteStream) => {
                    let auth = check_nip98_auth(&req, &self.settings, &self.db).await?;
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

        // Use streaming backend to generate endpoint URLs
        let backend = self.overseer.streaming_backend();
        let backend_endpoints = backend.get_ingest_endpoints(&user, &db_ingest_endpoints).await?;
        
        // Convert backend endpoints to API endpoints
        let endpoints: Vec<Endpoint> = backend_endpoints
            .into_iter()
            .map(|e| Endpoint {
                name: e.name,
                url: e.url,
                key: e.key,
                capabilities: e.capabilities,
                cost: EndpointCost {
                    unit: e.cost.unit,
                    rate: e.cost.rate,
                },
            })
            .collect();

        Ok(AccountInfo {
            endpoints,
            balance: user.balance / 1000,
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
            has_nwc: user.nwc.is_some(),
        })
    }

    async fn update_account(&self, pubkey: &PublicKey, account: PatchAccount) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        if let Some(accept_tos) = account.accept_tos
            && accept_tos
        {
            let user = self.db.get_user(uid).await?;
            if user.tos_accepted.is_none() {
                self.db.accept_tos(uid).await?;
            }
        }

        if let Some(url) = account.nwc
            && account.remove_nwc.is_none()
        {
            // test connection
            let parsed = NostrWalletConnectURI::parse(&url)?;
            let nwc = NWC::new(parsed);
            let info = nwc.get_info().await?;
            let perm = "pay_invoice".to_string();
            if !info.methods.contains(&perm) {
                bail!("NWC connection does not allow paying invoices!");
            }
            self.db.update_user_nwc(uid, Some(&url)).await?;
        }

        if let Some(x) = account.remove_nwc
            && x
        {
            self.db.update_user_nwc(uid, None).await?;
        }

        Ok(())
    }

    async fn topup(
        &self,
        pubkey: &PublicKey,
        amount_msats: usize,
        nostr: Option<String>,
    ) -> Result<TopupResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;

        let response = self
            .payments
            .add_invoice(AddInvoiceRequest {
                amount: amount_msats as _,
                memo: Some(format!("zap.stream topup for user {}", pubkey.to_hex())),
                expire: None,
            })
            .await?;

        let pr = response.pr();
        let r_hash = hex::decode(response.payment_hash())?;
        // Create payment entry for this topup invoice
        self.db
            .create_payment(
                &r_hash,
                uid,
                Some(&response.pr()),
                amount_msats as _,
                zap_stream_db::PaymentType::TopUp,
                0,
                DateTime::from_timestamp(
                    response.parsed_invoice.expires_at().unwrap().as_secs() as _,
                    0,
                )
                .unwrap(),
                nostr,
                response.external_id,
            )
            .await?;

        Ok(TopupResponse { pr })
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

    // TODO: broken
    #[cfg(all(feature = "withdrawal"))]
    async fn withdraw(&self, pubkey: &PublicKey, invoice: String) -> Result<WithdrawResponse> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let user = self.db.get_user(uid).await?;

        let mut lnd = self.lnd.clone();

        // Decode invoice to get amount and payment hash
        let decode_req = voltage_tonic_lnd::lnrpc::PayReqString {
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
                None,
            )
            .await?;

        // 3. Attempt Lightning payment
        let send_req = voltage_tonic_lnd::lnrpc::SendRequest {
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
        let history_entries = self.db.get_unified_user_history(uid, offset, limit).await?;

        let items: Vec<HistoryEntry> = history_entries
            .into_iter()
            .map(|entry| {
                let (entry_type, desc) = if let Some(payment_type) = entry.payment_type {
                    // This is a payment entry
                    let entry_type = match payment_type {
                        3 => 1, // Withdrawal = Debit (PaymentType::Withdrawal = 3)
                        _ => 0, // Credit (TopUp, Zap, Credit, AdmissionFee)
                    };
                    let desc = match payment_type {
                        3 => Some("Withdrawal".to_string()), // PaymentType::Withdrawal = 3
                        2 => Some("Admin Credit".to_string()), // PaymentType::Credit = 2
                        1 => entry.nostr, // PaymentType::Zap = 1, use nostr content
                        _ => None,
                    };
                    (entry_type, desc)
                } else {
                    // This is a stream entry
                    let desc = Some(format!(
                        "Stream: {}",
                        entry.stream_title.unwrap_or_else(|| entry
                            .stream_id
                            .unwrap_or_else(|| "Unknown".to_string()))
                    ));
                    (1, desc) // Debit
                };

                HistoryEntry {
                    created: entry.created.timestamp() as u64,
                    entry_type,
                    amount: entry.amount as f64 / 1000.0, // Convert from milli-sats to sats
                    desc,
                }
            })
            .collect();

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

        // Generate a new stream key first
        let key = Uuid::new_v4().to_string();

        // Create a new stream record for this key
        let stream_id = Uuid::new_v4();

        // Create the stream key record and get its ID
        let key_id = self
            .db
            .create_stream_key(uid, &key, req.expires, &stream_id.to_string())
            .await?;

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
            stream_key_id: Some(key_id),
            ..Default::default()
        };

        // Create the stream record with the stream_key_id set
        self.db.insert_stream(&new_stream).await?;

        // For now, return minimal response - event building would require nostr integration
        Ok(CreateStreamKeyResponse {
            key,
            event: None, // TODO: Build proper nostr event like C# version
        })
    }

    async fn check_admin_access(&self, pubkey: &PublicKey) -> Result<u64> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let is_admin = self.db.is_admin(uid).await?;
        if !is_admin {
            bail!("Access denied: Admin privileges required");
        }
        Ok(uid)
    }

    async fn delete_stream(&self, pubkey: &PublicKey, stream_id: &str) -> Result<()> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let stream_uuid = Uuid::parse_str(stream_id)?;
        let stream = self.db.get_stream(&stream_uuid).await?;

        // Verify the user owns this stream OR is an admin
        let is_admin = self.db.is_admin(uid).await?;
        if stream.user_id != uid && !is_admin {
            bail!("Access denied: You can only delete your own streams");
        }

        // Publish Nostr deletion request event if the stream has an associated event
        if let Some(event_json) = &stream.event
            && let Ok(stream_event) = serde_json::from_str::<nostr_sdk::Event>(event_json)
        {
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

        // Log admin action if this is an admin deleting someone else's stream
        if is_admin && stream.user_id != uid {
            let message = format!(
                "Admin deleted stream {} belonging to user {}",
                stream_id, stream.user_id
            );
            let metadata = serde_json::json!({
                "target_stream_id": stream_id,
                "target_user_id": stream.user_id,
                "stream_title": stream.title
            });
            self.db
                .log_admin_action(
                    uid,
                    "delete_stream",
                    Some("stream"),
                    Some(stream_id),
                    &message,
                    Some(&metadata.to_string()),
                )
                .await?;
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
                stream_dump_recording: user.stream_dump_recording,
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

    async fn admin_manage_user(
        &self,
        admin_uid: u64,
        user_id: &str,
        req: AdminUserRequest,
    ) -> Result<()> {
        let uid: u64 = user_id.parse()?;

        if let Some(is_admin) = req.set_admin {
            self.db.set_admin(uid, is_admin).await?;

            // Log admin action
            let action = if is_admin {
                "grant_admin"
            } else {
                "revoke_admin"
            };
            let message = format!(
                "Admin status {} for user {}",
                if is_admin {
                    "granted to"
                } else {
                    "revoked from"
                },
                uid
            );
            let metadata = serde_json::json!({
                "target_user_id": uid,
                "admin_status": is_admin
            });
            self.db
                .log_admin_action(
                    admin_uid,
                    action,
                    Some("user"),
                    Some(&uid.to_string()),
                    &message,
                    Some(&metadata.to_string()),
                )
                .await?;
        }

        if let Some(is_blocked) = req.set_blocked {
            self.db.set_blocked(uid, is_blocked).await?;

            // If blocking the user, stop all their current streams
            if is_blocked {
                let live_streams = self.db.get_user_live_streams(uid).await?;
                let mut stopped_streams = Vec::new();
                
                for stream in live_streams {
                    let stream_uuid = match Uuid::parse_str(&stream.id) {
                        Ok(id) => id,
                        Err(e) => {
                            warn!("Failed to parse stream ID {} as UUID: {}", stream.id, e);
                            continue;
                        }
                    };
                    
                    if let Err(e) = self.overseer.on_end(&stream_uuid).await {
                        error!("Failed to stop stream {} for blocked user {}: {}", stream.id, uid, e);
                    } else {
                        info!("Stopped stream {} for blocked user {}", stream.id, uid);
                        stopped_streams.push(stream.id);
                    }
                }

                // Log admin action with stopped streams information
                let action = "block_user";
                let message = format!(
                    "User {} blocked, {} stream(s) stopped",
                    uid,
                    stopped_streams.len()
                );
                let metadata = serde_json::json!({
                    "target_user_id": uid,
                    "blocked_status": true,
                    "stopped_streams": stopped_streams
                });
                self.db
                    .log_admin_action(
                        admin_uid,
                        action,
                        Some("user"),
                        Some(&uid.to_string()),
                        &message,
                        Some(&metadata.to_string()),
                    )
                    .await?;
            } else {
                // Just log unblock action
                let action = "unblock_user";
                let message = format!("User {} unblocked", uid);
                let metadata = serde_json::json!({
                    "target_user_id": uid,
                    "blocked_status": false
                });
                self.db
                    .log_admin_action(
                        admin_uid,
                        action,
                        Some("user"),
                        Some(&uid.to_string()),
                        &message,
                        Some(&metadata.to_string()),
                    )
                    .await?;
            }
        }

        if let Some(enable_stream_dump_recording) = req.set_stream_dump_recording {
            self.db
                .set_stream_dump_recording(uid, enable_stream_dump_recording)
                .await?;

            // Log admin action
            let action = if enable_stream_dump_recording {
                "enable_stream_dump_recording"
            } else {
                "disable_stream_dump_recording"
            };
            let message = format!(
                "Stream dump recording {} for user {}",
                if enable_stream_dump_recording {
                    "enabled"
                } else {
                    "disabled"
                },
                uid
            );
            let metadata = serde_json::json!({
                "target_user_id": uid,
                "stream_dump_recording": enable_stream_dump_recording
            });
            self.db
                .log_admin_action(
                    admin_uid,
                    action,
                    Some("user"),
                    Some(&uid.to_string()),
                    &message,
                    Some(&metadata.to_string()),
                )
                .await?;
        }

        if let Some(credit_amount) = req.add_credit
            && credit_amount > 0
        {
            self.db
                .add_admin_credit(uid, credit_amount, req.memo.as_deref())
                .await?;

            // Log admin action
            let message = format!("Added {} credits to user {}", credit_amount, uid);
            let metadata = serde_json::json!({
                "target_user_id": uid,
                "credit_amount": credit_amount,
                "memo": req.memo
            });
            self.db
                .log_admin_action(
                    admin_uid,
                    "add_credit",
                    Some("user"),
                    Some(&uid.to_string()),
                    &message,
                    Some(&metadata.to_string()),
                )
                .await?;
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

            // Log admin action
            let message = format!("Updated default stream settings for user {}", uid);
            let metadata = serde_json::json!({
                "target_user_id": uid,
                "title": req.title,
                "summary": req.summary,
                "image": req.image,
                "tags": req.tags,
                "content_warning": req.content_warning,
                "goal": req.goal
            });
            self.db
                .log_admin_action(
                    admin_uid,
                    "update_user_defaults",
                    Some("user"),
                    Some(&uid.to_string()),
                    &message,
                    Some(&metadata.to_string()),
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

    async fn admin_get_user_stream_key(
        &self,
        user_id: u64,
        admin_uid: u64,
    ) -> Result<AdminStreamKeyResponse> {
        let user = self.db.get_user(user_id).await?;

        // Log the admin action
        self.db
            .log_admin_action(
                admin_uid,
                "view_stream_key",
                Some("user"),
                Some(&user_id.to_string()),
                &format!("Admin viewed stream key for user {}", user_id),
                Some(&format!(r#"{{"target_user_id": {}}}"#, user_id)),
            )
            .await?;

        Ok(AdminStreamKeyResponse {
            stream_key: user.stream_key,
        })
    }

    async fn admin_regenerate_user_stream_key(
        &self,
        admin_uid: u64,
        user_id: u64,
    ) -> Result<AdminStreamKeyResponse> {
        // Generate a new UUID for the stream key
        let new_key = Uuid::new_v4().to_string();

        // Update the user's main stream key
        self.db.update_user_stream_key(user_id, &new_key).await?;

        // Log admin action
        let message = format!("Regenerated stream key for user {}", user_id);
        let metadata = serde_json::json!({
            "target_user_id": user_id,
            "new_key": new_key
        });
        self.db
            .log_admin_action(
                admin_uid,
                "regenerate_stream_key",
                Some("user"),
                Some(&user_id.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(AdminStreamKeyResponse {
            stream_key: new_key,
        })
    }

    async fn admin_get_audit_logs(&self, page: u64, limit: u64) -> Result<AdminAuditLogResponse> {
        let offset = page * limit;
        let (logs, total) = self.db.get_audit_logs_with_pubkeys(offset, limit).await?;

        let logs_info: Vec<AdminAuditLogEntry> = logs
            .into_iter()
            .map(|log| AdminAuditLogEntry {
                id: log.id,
                admin_id: log.admin_id,
                admin_pubkey: Some(hex::encode(log.admin_pubkey)),
                action: log.action,
                target_type: log.target_type,
                target_id: log.target_id,
                target_pubkey: log.target_pubkey.map(hex::encode),
                message: log.message,
                metadata: log
                    .metadata
                    .map(|a| String::from_utf8_lossy(&a).to_string()),
                created: log.created.timestamp() as u64,
            })
            .collect();

        Ok(AdminAuditLogResponse {
            logs: logs_info,
            page: page as u32,
            limit: limit as u32,
            total: total as u32,
        })
    }

    async fn admin_list_ingest_endpoints(
        &self,
        _admin_uid: u64,
        page: u64,
        limit: u64,
    ) -> Result<AdminIngestEndpointsResponse> {
        let offset = page * limit;
        let endpoints = self.db.get_ingest_endpoints().await?;
        let total = endpoints.len() as u64;

        let paginated_endpoints = endpoints
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|endpoint| AdminIngestEndpointResponse {
                id: endpoint.id,
                name: endpoint.name.clone(),
                cost: endpoint.cost,
                capabilities: endpoint
                    .capabilities
                    .map(|c| c.split(',').map(|s| s.trim().to_string()).collect()),
                urls: self.generate_endpoint_urls(&endpoint.name),
            })
            .collect();

        Ok(AdminIngestEndpointsResponse {
            endpoints: paginated_endpoints,
            page: page as u32,
            limit: limit as u32,
            total: total as u32,
        })
    }

    async fn admin_create_ingest_endpoint(
        &self,
        admin_uid: u64,
        req: AdminIngestEndpointRequest,
    ) -> Result<AdminIngestEndpointResponse> {
        let capabilities_str = req.capabilities.as_ref().map(|caps| caps.join(","));
        let endpoint_id = self
            .db
            .create_ingest_endpoint(&req.name, req.cost, capabilities_str.as_deref())
            .await?;

        // Log admin action
        let message = format!("Created ingest endpoint: {} (cost: {})", req.name, req.cost);
        let metadata = serde_json::json!({
            "endpoint_id": endpoint_id,
            "name": req.name,
            "cost": req.cost,
            "capabilities": req.capabilities
        });
        self.db
            .log_admin_action(
                admin_uid,
                "create_ingest_endpoint",
                Some("ingest_endpoint"),
                Some(&endpoint_id.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(AdminIngestEndpointResponse {
            id: endpoint_id,
            name: req.name.clone(),
            cost: req.cost,
            capabilities: req.capabilities,
            urls: self.generate_endpoint_urls(&req.name),
        })
    }

    async fn admin_get_ingest_endpoint(
        &self,
        _admin_uid: u64,
        endpoint_id: u64,
    ) -> Result<AdminIngestEndpointResponse> {
        let endpoint = self.db.get_ingest_endpoint(endpoint_id).await?;

        Ok(AdminIngestEndpointResponse {
            id: endpoint.id,
            name: endpoint.name.clone(),
            cost: endpoint.cost,
            capabilities: endpoint
                .capabilities
                .map(|c| c.split(',').map(|s| s.trim().to_string()).collect()),
            urls: self.generate_endpoint_urls(&endpoint.name),
        })
    }

    async fn admin_update_ingest_endpoint(
        &self,
        admin_uid: u64,
        endpoint_id: u64,
        req: AdminIngestEndpointRequest,
    ) -> Result<AdminIngestEndpointResponse> {
        let capabilities_str = req.capabilities.as_ref().map(|caps| caps.join(","));
        self.db
            .update_ingest_endpoint(
                endpoint_id,
                &req.name,
                req.cost,
                capabilities_str.as_deref(),
            )
            .await?;

        // Log admin action
        let message = format!(
            "Updated ingest endpoint {}: {} (cost: {})",
            endpoint_id, req.name, req.cost
        );
        let metadata = serde_json::json!({
            "endpoint_id": endpoint_id,
            "name": req.name,
            "cost": req.cost,
            "capabilities": req.capabilities
        });
        self.db
            .log_admin_action(
                admin_uid,
                "update_ingest_endpoint",
                Some("ingest_endpoint"),
                Some(&endpoint_id.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(AdminIngestEndpointResponse {
            id: endpoint_id,
            name: req.name.clone(),
            cost: req.cost,
            capabilities: req.capabilities,
            urls: self.generate_endpoint_urls(&req.name),
        })
    }

    async fn admin_delete_ingest_endpoint(&self, admin_uid: u64, endpoint_id: u64) -> Result<()> {
        // Get the endpoint first for logging
        let endpoint = self.db.get_ingest_endpoint(endpoint_id).await?;

        // Delete the endpoint
        self.db.delete_ingest_endpoint(endpoint_id).await?;

        // Log admin action
        let message = format!("Deleted ingest endpoint {}: {}", endpoint_id, endpoint.name);
        let metadata = serde_json::json!({
            "endpoint_id": endpoint_id,
            "name": endpoint.name,
            "cost": endpoint.cost,
            "capabilities": endpoint.capabilities
        });
        self.db
            .log_admin_action(
                admin_uid,
                "delete_ingest_endpoint",
                Some("ingest_endpoint"),
                Some(&endpoint_id.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(())
    }

    async fn admin_get_pipeline_log(&self, admin_uid: u64, stream_id: &str) -> Result<String> {
        use tokio::fs;

        // Validate stream_id is a valid UUID to prevent path traversal attacks
        let stream_uuid =
            Uuid::parse_str(stream_id).context("Invalid stream_id format, must be a valid UUID")?;

        // Construct path to pipeline.log in stream's output directory
        // Using the parsed UUID's string representation ensures it's sanitized
        let log_path = std::path::Path::new(&self.settings.output_dir)
            .join(stream_uuid.to_string())
            .join("pipeline.log");

        // Try to read the log file
        let log_content = match fs::read_to_string(&log_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Return helpful message if file doesn't exist
                String::from(
                    "Pipeline log file not found. This may be because the stream has not been started yet or the stream ID is invalid.",
                )
            }
            Err(e) => {
                // Return error for other IO errors
                bail!("Failed to read pipeline log: {}", e);
            }
        };

        // Log admin action
        self.db
            .log_admin_action(
                admin_uid,
                "view_pipeline_log",
                Some("stream"),
                Some(stream_id),
                &format!("Admin viewed pipeline log for stream {}", stream_id),
                None,
            )
            .await?;

        Ok(log_content)
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
            self.settings.clone(),
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

    fn handle_webhook(
        &self,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        let backend = self.overseer.streaming_backend();
        let overseer = self.overseer.clone();
        
        Box::pin(async move {
            // Parse the webhook payload using the backend
            let event = backend.parse_external_event(&payload)?;
            
            if let Some(event) = event {
                use crate::streaming_backend::ExternalStreamEvent;
                
                match event {
                    ExternalStreamEvent::Connected { connection_info } => {
                        info!("Webhook: Stream connected - {}", connection_info.id);
                        
                        // For cloud backends, we don't have IngressInfo from webhooks
                        // Pass None to indicate no local pipeline should be created
                        match overseer.start_stream(&connection_info, None).await {
                            Ok(_) => info!("Stream started successfully via webhook: {}", connection_info.id),
                            Err(e) => error!("Failed to start stream via webhook: {}", e),
                        }
                    }
                    ExternalStreamEvent::Disconnected { stream_id } => {
                        info!("Webhook: Stream disconnected - {}", stream_id);
                        
                        // Trigger the overseer's stream end logic
                        match overseer.on_end(&stream_id).await {
                            Ok(_) => info!("Stream ended successfully via webhook: {}", stream_id),
                            Err(e) => error!("Failed to end stream via webhook: {}", e),
                        }
                    }
                }
            }
            
            Ok(())
        })
    }
}

#[derive(Deserialize, Serialize)]
struct AccountInfo {
    pub endpoints: Vec<Endpoint>,
    pub balance: i64,
    pub tos: AccountTos,
    pub forwards: Vec<ForwardDest>,
    pub details: Option<PatchEventDetails>,
    pub has_nwc: bool,
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
    /// Accept TOS
    pub accept_tos: Option<bool>,
    /// Configure a new NWC
    pub nwc: Option<String>,
    /// Remove configured NWC
    pub remove_nwc: Option<bool>,
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
    pub stream_dump_recording: bool,
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
    pub set_stream_dump_recording: Option<bool>,
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

#[derive(Deserialize, Serialize)]
struct AdminStreamKeyResponse {
    pub stream_key: String,
}

#[derive(Deserialize, Serialize)]
struct AdminAuditLogEntry {
    pub id: u64,
    pub admin_id: u64,
    pub admin_pubkey: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub target_pubkey: Option<String>,
    pub message: String,
    pub metadata: Option<String>,
    pub created: u64,
}

#[derive(Deserialize, Serialize)]
struct AdminAuditLogResponse {
    pub logs: Vec<AdminAuditLogEntry>,
    pub page: u32,
    pub limit: u32,
    pub total: u32,
}

#[derive(Deserialize, Serialize)]
struct AdminIngestEndpointRequest {
    pub name: String,
    pub cost: u64,
    pub capabilities: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize)]
struct AdminIngestEndpointResponse {
    pub id: u64,
    pub name: String,
    pub cost: u64,
    pub capabilities: Option<Vec<String>>,
    pub urls: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct AdminIngestEndpointsResponse {
    pub endpoints: Vec<AdminIngestEndpointResponse>,
    pub page: u32,
    pub limit: u32,
    pub total: u32,
}

#[derive(Deserialize, Serialize)]
struct TimeResponse {
    pub time: u64,
}
