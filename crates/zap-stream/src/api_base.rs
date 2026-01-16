use crate::user_history_to_api_model;
use anyhow::Result;
use anyhow::bail;
use chrono::{DateTime, Utc};
use nostr_sdk::Client;
use nostr_sdk::prelude::{EventDeletionRequest, NostrWalletConnectUri};
use nwc::NostrWalletConnect;
use payments_rs::lightning::{AddInvoiceRequest, LightningNode};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;
use zap_stream_api_common::{
    CreateStreamKeyRequest, CreateStreamKeyResponse, HistoryEntry, HistoryResponse, Nip98Auth,
    PatchAccount, PatchEvent, StreamKey, TopupResponse,
};
use zap_stream_db::ZapStreamDb;

/// Basic API implementation which covers the simple database updates
#[derive(Clone)]
pub struct ApiBase {
    db: ZapStreamDb,
    client: Client,
    lightning: Arc<dyn LightningNode>,
}

impl ApiBase {
    pub fn new(db: ZapStreamDb, client: Client, lightning: Arc<dyn LightningNode>) -> Self {
        Self {
            db,
            client,
            lightning,
        }
    }

    pub async fn update_account(&self, auth: Nip98Auth, patch_account: PatchAccount) -> Result<()> {
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

    pub async fn update_event(&self, auth: Nip98Auth, patch: PatchEvent) -> Result<()> {
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

    pub async fn delete_event(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()> {
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

            if let Err(e) = self.client.send_event_builder(deletion_event).await {
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

    pub async fn get_balance_history(
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

    pub async fn get_stream_keys(&self, auth: Nip98Auth) -> Result<Vec<StreamKey>> {
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

    pub async fn create_stream_key(
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

    pub async fn topup(
        &self,
        pubkey: [u8; 32],
        amount_msats: u64,
        zap: Option<String>,
    ) -> Result<TopupResponse> {
        let uid = self.db.upsert_user(&pubkey).await?;

        let response = self
            .lightning
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
}
