use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nostr_sdk::Client;
use nostr_sdk::prelude::EventDeletionRequest;
use nwc::prelude::{NostrWalletConnect, NostrWalletConnectUri};
use payments_rs::lightning::{AddInvoiceRequest, LightningNode};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;
use zap_stream::user_history_to_api_model;
use zap_stream_api_common::{
    AccountInfo, AccountTos, CreateStreamKeyRequest, CreateStreamKeyResponse, Endpoint,
    EndpointCost, ForwardDest, ForwardRequest, ForwardResponse, GameDb, GameInfo, HistoryEntry,
    HistoryResponse, Nip98Auth, PatchAccount, PatchEvent, PatchEventDetails, StreamKey,
    TopupResponse, UpdateForwardRequest, ZapStreamApi,
};
use zap_stream_core::listen::ListenerEndpoint;
use zap_stream_core::overseer::Overseer;
use zap_stream_db::{IngestEndpoint, ZapStreamDb};

#[derive(Clone)]
pub struct Api {
    db: ZapStreamDb,
    settings: Settings,
    payments: Arc<dyn LightningNode>,
    overseer: Arc<dyn Overseer>,
    nostr_client: Client,
    game_db: GameDb,
}

impl Api {
    pub fn new(overseer: Arc<ZapStreamOverseer>, settings: Settings) -> Self {
        //router.insert("/metrics", Route::Metrics).unwrap();
        Self {
            db: overseer.database(),
            game_db: GameDb::new(settings.twitch.clone()),
            settings,
            payments: overseer.lightning(),
            nostr_client: overseer.nostr_client(),
            overseer,
        }
    }

    fn create_endpoints(&self, endpoints: &Vec<IngestEndpoint>, stream_key: &str) -> Vec<Endpoint> {
        let mut res = Vec::new();
        for setting_endpoint in &self.settings.endpoints {
            if let Ok(listener_endpoint) = ListenerEndpoint::from_str(setting_endpoint) {
                for ingest in endpoints {
                    if let Some(url) = listener_endpoint.to_public_url(
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
        })
    }

    async fn update_account(&self, auth: Nip98Auth, patch_account: PatchAccount) -> Result<()> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;

        if let Some(accept_tos) = patch_account.accept_tos
            && accept_tos
        {
            let user = self.db.get_user(uid).await?;
            if user.tos_accepted.is_none() {
                self.db.accept_tos(uid).await?;
            }
        }

        if let Some(url) = patch_account.nwc
            && patch_account.remove_nwc.is_none()
        {
            // test connection
            let parsed = NostrWalletConnectUri::parse(&url)?;
            let nwc = NostrWalletConnect::new(parsed);
            let info = nwc.get_info().await?;
            if !info.methods.contains(&nwc::prelude::Method::PayInvoice) {
                bail!("NWC connection does not allow paying invoices!");
            }
            self.db.update_user_nwc(uid, Some(&url)).await?;
        }

        if let Some(x) = patch_account.remove_nwc
            && x
        {
            self.db.update_user_nwc(uid, None).await?;
        }

        Ok(())
    }

    async fn update_event(&self, auth: Nip98Auth, patch: PatchEvent) -> Result<()> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;

        if patch.id.as_ref().map(|i| !i.is_empty()).unwrap_or(false) {
            // Update specific stream
            let stream_uuid = Uuid::parse_str(&patch.id.unwrap())?;
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
            if let Some(title) = patch.title {
                stream.title = Some(title);
            }
            if let Some(summary) = patch.summary {
                stream.summary = Some(summary);
            }
            if let Some(image) = patch.image {
                stream.image = Some(image);
            }
            if let Some(tags) = patch.tags {
                stream.tags = Some(tags.join(","));
            }
            if let Some(content_warning) = patch.content_warning {
                stream.content_warning = Some(content_warning);
            }
            if let Some(goal) = patch.goal {
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
                    patch.title.as_deref(),
                    patch.summary.as_deref(),
                    patch.image.as_deref(),
                    patch.tags.as_ref().map(|t| t.join(",")).as_deref(),
                    patch.content_warning.as_deref(),
                    patch.goal.as_deref(),
                )
                .await?;
        }

        Ok(())
    }

    async fn delete_event(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let stream = self.db.get_stream(&stream_id).await?;

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
                    Some(&stream_id.to_string()),
                    &message,
                    Some(&metadata.to_string()),
                )
                .await?;
        }

        Ok(())
    }

    async fn create_forward(
        &self,
        auth: Nip98Auth,
        req: ForwardRequest,
    ) -> Result<ForwardResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let forward_id = self.db.create_forward(uid, &req.name, &req.target).await?;

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
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let offset = page * page_size;
        let history_entries = self
            .db
            .get_unified_user_history(uid, offset as _, page_size as _)
            .await?;

        let items: Vec<HistoryEntry> = history_entries
            .into_iter()
            .map(user_history_to_api_model)
            .collect();

        Ok(HistoryResponse {
            items,
            page: page as i32,
            page_size: page_size as i32,
        })
    }

    async fn get_stream_keys(&self, auth: Nip98Auth) -> Result<Vec<StreamKey>> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
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
        auth: Nip98Auth,
        req: CreateStreamKeyRequest,
    ) -> Result<CreateStreamKeyResponse> {
        let uid = self.db.upsert_user(&auth.pubkey).await?;
        let new_key = Uuid::new_v4().to_string();
        let stream_id = Uuid::new_v4();
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

        // Create the stream record with the stream_key_id set
        self.db.insert_stream(&new_stream).await?;

        // Create the stream key record and get its ID
        let key_id = self
            .db
            .create_stream_key(uid, &new_key, req.expires, &stream_id.to_string())
            .await?;

        // set the stream key id on the stream event
        new_stream.stream_key_id = Some(key_id);
        self.db.update_stream(&new_stream).await?;

        // For now, return minimal response - event building would require nostr integration
        Ok(CreateStreamKeyResponse {
            key: new_key,
            event: None, // TODO: Build proper nostr event like C# version
        })
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
        let uid = self.db.upsert_user(&pubkey).await?;

        let response = self
            .payments
            .add_invoice(AddInvoiceRequest {
                amount: amount_msats as _,
                memo: Some(format!("zap.stream topup for user {}", hex::encode(pubkey))),
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
                zap,
                response.external_id,
            )
            .await?;

        Ok(TopupResponse { pr })
    }

    async fn search_games(&self, q: String) -> Result<Vec<GameInfo>> {
        self.game_db.search_games(&q, 10).await
    }

    async fn get_game(&self, id: String) -> Result<GameInfo> {
        self.game_db.get_game(&id).await
    }
}
