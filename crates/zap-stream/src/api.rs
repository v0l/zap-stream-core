use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::str::FromStr;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;
use zap_stream::api_base::ApiBase;
use zap_stream_api_common::{
    AccountInfo, AccountTos, CreateStreamKeyRequest, CreateStreamKeyResponse, Endpoint,
    EndpointCost, ForwardDest, ForwardRequest, ForwardResponse, GameDb, GameInfo, HistoryResponse,
    Nip98Auth, PatchAccount, PatchEvent, PatchEventDetails, StreamKey, TopupResponse,
    UpdateForwardRequest, ZapStreamApi,
};
use zap_stream_core::listen::ListenerEndpoint;
use zap_stream_core::overseer::Overseer;
use zap_stream_db::{IngestEndpoint, ZapStreamDb};

#[derive(Clone)]
pub struct Api {
    db: ZapStreamDb,
    settings: Settings,
    overseer: Arc<dyn Overseer>,
    game_db: Option<GameDb>,
    api_base: ApiBase,
}

impl Api {
    pub fn new(overseer: Arc<ZapStreamOverseer>, settings: Settings) -> Self {
        //router.insert("/metrics", Route::Metrics).unwrap();
        Self {
            db: overseer.database(),
            game_db: settings.twitch.as_ref().map(|t| GameDb::new(t.clone())),
            settings,
            api_base: ApiBase::new(
                overseer.database(),
                overseer.nostr_client(),
                overseer.lightning(),
            ),
            overseer,
        }
    }

    fn create_endpoints(&self, endpoints: &Vec<IngestEndpoint>, stream_key: &str) -> Vec<Endpoint> {
        let mut res = Vec::new();
        for setting_endpoint in &self.settings.endpoints {
            if let Ok(listener_endpoint) = ListenerEndpoint::from_str(setting_endpoint) {
                for ingest in endpoints {
                    if let Ok(url) = listener_endpoint.to_public_url(
                        &self.settings.endpoints_public_hostname,
                        &ingest.name.to_lowercase(),
                    ) {
                        let protocol = match listener_endpoint {
                            ListenerEndpoint::SRT { .. } => "SRT",
                            ListenerEndpoint::RTMP { .. } => "RTMP",
                            ListenerEndpoint::TCP { .. } => "TCP",
                            _ => continue,
                        };

                        res.push(Endpoint {
                            name: format!("{}-{}", protocol, ingest.name),
                            url,
                            key: stream_key.to_string(),
                            capabilities: ingest
                                .capabilities
                                .as_ref()
                                .map(|c| c.split(',').map(|s| s.trim().to_string()).collect())
                                .unwrap_or_else(Vec::new),
                            cost: EndpointCost {
                                unit: "min".to_string(),
                                rate: ingest.cost as f32 / 1000.0,
                            },
                        });
                    }
                }
            }
        }
        res
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
}

#[async_trait]
impl ZapStreamApi for Api {
    async fn get_account(&self, auth: Nip98Auth) -> Result<AccountInfo> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let user = self.db.get_user(uid).await?;

        // Get user forwards
        let forwards = self.db.get_user_forwards(uid).await?;
        let ingest_endpoints = self.db.get_ingest_endpoints().await?;

        Ok(AccountInfo {
            endpoints: self.create_endpoints(&ingest_endpoints, &user.stream_key),
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
            armed_stream_id: user.armed_stream_id,
        })
    }

    async fn update_account(&self, auth: Nip98Auth, patch_account: PatchAccount) -> Result<()> {
        self.api_base.update_account(auth, patch_account).await
    }

    async fn update_event(&self, auth: Nip98Auth, patch: PatchEvent) -> Result<()> {
        if self.api_base.update_event(auth, patch.clone()).await?
            && let Some(id) = patch.id
            && let Ok(uuid) = id.parse()
        {
            if let Err(e) = self.overseer.on_update(&uuid).await {
                warn!("Failed to republish nostr event for stream {}: {}", uuid, e);
            }
        }
        Ok(())
    }

    async fn delete_event(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()> {
        self.api_base.delete_event(auth, stream_id).await
    }

    async fn arm_stream(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()> {
        self.api_base.arm_stream(auth, stream_id).await
    }

    async fn disarm_stream(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()> {
        self.api_base.disarm_stream(auth, stream_id).await
    }

    async fn create_forward(
        &self,
        auth: Nip98Auth,
        req: ForwardRequest,
    ) -> Result<ForwardResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let forward_id = self
            .db
            .create_forward(uid, &req.name, &req.target, None)
            .await?;
        Ok(ForwardResponse { id: forward_id })
    }

    async fn delete_forward(&self, auth: Nip98Auth, forward_id: u64) -> Result<()> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        self.db.delete_forward(uid, forward_id).await?;
        Ok(())
    }

    async fn update_forward(
        &self,
        auth: Nip98Auth,
        forward_id: u64,
        req: UpdateForwardRequest,
    ) -> Result<ForwardResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        self.db
            .update_forward_disabled(uid, forward_id, req.disabled)
            .await?;
        Ok(ForwardResponse { id: forward_id })
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
        self.api_base.get_stream_keys(auth).await
    }

    async fn create_stream_key(
        &self,
        auth: Nip98Auth,
        req: CreateStreamKeyRequest,
    ) -> Result<CreateStreamKeyResponse> {
        let (mut rsp, announce) = self.api_base.create_stream_key(auth, req).await?;
        if let Some(stream_id) = announce {
            match self.overseer.on_update(&stream_id).await {
                Ok(()) => rsp.event = self.db.get_stream(&stream_id).await?.event,
                Err(e) => warn!(
                    "Failed to publish nostr event for stream {}: {}",
                    stream_id, e
                ),
            }
        }
        Ok(rsp)
    }

    async fn delete_stream_key(&self, _auth: Nip98Auth, _key_id: u64) -> Result<()> {
        todo!()
    }

    async fn topup(
        &self,
        pubkey: [u8; 32],
        amount_msats: u64,
        zap: Option<String>,
    ) -> Result<TopupResponse> {
        self.api_base.topup(pubkey, amount_msats, zap).await
    }

    async fn search_games(&self, q: String) -> Result<Vec<GameInfo>> {
        if let Some(db) = &self.game_db {
            db.search_games(&q, 10).await
        } else {
            bail!("Twitch api keys not configured")
        }
    }

    async fn get_game(&self, id: String) -> Result<GameInfo> {
        if let Some(db) = &self.game_db {
            db.get_game(&id).await
        } else {
            bail!("Twitch api keys not configured")
        }
    }
}

#[cfg(all(test, feature = "pipeline"))]
mod tests {
    use super::*;
    use crate::test_support::{TestDb, nip98_header, test_overseer, test_settings};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::{Duration, Utc};
    use http_body_util::BodyExt;
    use nostr_sdk::Keys;
    use tower::ServiceExt;
    use zap_stream_api_common::AxumApi;
    use zap_stream_db::{UserStream, UserStreamState};

    struct Harness {
        t: TestDb,
        router: axum::Router,
    }

    async fn harness() -> Harness {
        let t = TestDb::start().await;
        let overseer = test_overseer(&t.db);
        let router = AxumApi::new(Api::new(overseer, test_settings()));
        Harness { t, router }
    }

    async fn call(h: &Harness, keys: &Keys, method: &str, path: &str) -> (StatusCode, String) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                nip98_header(keys, method, &format!("http://localhost{path}")),
            )
            .body(Body::empty())
            .unwrap();
        let rsp = h.router.clone().oneshot(req).await.unwrap();
        let status = rsp.status();
        let body = rsp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).to_string())
    }

    async fn stream_for(h: &Harness, keys: &Keys, state: UserStreamState) -> Uuid {
        let uid = h
            .t
            .db
            .upsert_user(&keys.public_key().to_bytes())
            .await
            .unwrap();
        let s = UserStream {
            id: Uuid::new_v4().to_string(),
            user_id: uid,
            starts: Utc::now() + Duration::hours(1),
            state,
            ..Default::default()
        };
        h.t.db.insert_stream(&s).await.unwrap();
        Uuid::parse_str(&s.id).unwrap()
    }

    fn arm_path(id: &Uuid) -> String {
        format!("/api/v1/planned/{id}/arm")
    }

    #[tokio::test]
    async fn owner_arms_and_account_shows_it() {
        let h = harness().await;
        let keys = Keys::generate();
        let id = stream_for(&h, &keys, UserStreamState::Planned).await;

        let (status, _) = call(&h, &keys, "POST", &arm_path(&id)).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = call(&h, &keys, "GET", "/api/v1/account").await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["armed_stream_id"], id.to_string());
    }

    #[tokio::test]
    async fn owner_disarms() {
        let h = harness().await;
        let keys = Keys::generate();
        let id = stream_for(&h, &keys, UserStreamState::Planned).await;
        call(&h, &keys, "POST", &arm_path(&id)).await;

        let (status, _) = call(&h, &keys, "DELETE", &arm_path(&id)).await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = call(&h, &keys, "GET", "/api/v1/account").await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["armed_stream_id"].is_null());
    }

    #[tokio::test]
    async fn non_owner_gets_403() {
        let h = harness().await;
        let owner = Keys::generate();
        let id = stream_for(&h, &owner, UserStreamState::Planned).await;

        let (status, _) = call(&h, &Keys::generate(), "POST", &arm_path(&id)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_stream_gets_404() {
        let h = harness().await;

        let (status, _) = call(&h, &Keys::generate(), "POST", &arm_path(&Uuid::new_v4())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn live_and_ended_streams_get_409() {
        let h = harness().await;
        let keys = Keys::generate();
        for state in [UserStreamState::Live, UserStreamState::Ended] {
            let id = stream_for(&h, &keys, state).await;
            let (status, _) = call(&h, &keys, "POST", &arm_path(&id)).await;
            assert_eq!(status, StatusCode::CONFLICT);
        }
    }
}
