use crate::user_history_to_api_model;
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;
use zap_stream_api_common::*;
use zap_stream_core::listen::ListenerEndpoint;
use zap_stream_db::{IngestEndpoint, ZapStreamDb};

#[derive(Clone)]
pub struct ZapStreamAdminApiImpl {
    db: ZapStreamDb,
    /// Output directory for pipeline data
    output_dir: PathBuf,
    /// List of listener endpoints
    endpoints: Vec<String>,
    /// Hostname which points directly to the listener endpoints
    endpoints_public_hostname: String,
}

impl ZapStreamAdminApiImpl {
    pub fn new(
        db: ZapStreamDb,
        output_dir: PathBuf,
        endpoints: Vec<String>,
        endpoints_public_hostname: String,
    ) -> Self {
        Self {
            db,
            output_dir,
            endpoints,
            endpoints_public_hostname,
        }
    }

    fn map_user_to_admin_user(user: &zap_stream_db::User) -> AdminUserInfo {
        AdminUserInfo {
            id: user.id,
            pubkey: hex::encode(&user.pubkey),
            created: user.created.timestamp() as u64,
            balance: user.balance,
            is_admin: user.is_admin,
            is_blocked: user.is_blocked,
            stream_dump_recording: user.stream_dump_recording,
            tos_accepted: user.tos_accepted.map(|t| t.timestamp() as u64),
            title: user.title.clone(),
            summary: user.summary.clone(),
        }
    }

    async fn check_admin_access(&self, pubkey: &[u8; 32]) -> Result<u64> {
        if let Some(uid) = self.db.get_admin_uid(pubkey).await? {
            return Ok(uid);
        }
        bail!("Access denied: Admin privileges required");
    }

    fn generate_endpoint_urls(&self, name: &str) -> Vec<String> {
        self.endpoints
            .iter()
            .filter_map(|endpoint_url| {
                ListenerEndpoint::from_str(endpoint_url)
                    .ok()
                    .and_then(|e| e.to_public_url(&self.endpoints_public_hostname, name).ok())
            })
            .collect()
    }

    fn map_endpoint_to_admin_endpoint(
        &self,
        endpoint: IngestEndpoint,
    ) -> AdminIngestEndpointResponse {
        AdminIngestEndpointResponse {
            id: endpoint.id,
            name: endpoint.name.clone(),
            cost: endpoint.cost,
            capabilities: endpoint
                .capabilities
                .map(|c| c.split(',').map(|s| s.trim().to_string()).collect()),
            urls: self.generate_endpoint_urls(&endpoint.name),
        }
    }
}

#[async_trait]
impl ZapStreamAdminApi for ZapStreamAdminApiImpl {
    async fn get_users(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
        search: Option<String>,
    ) -> Result<AdminUsersResponse> {
        self.check_admin_access(&auth.pubkey).await?;
        let offset = page * page_size;

        let (users, total) = if let Some(search_term) = search {
            self.db.search_users_by_pubkey(&search_term).await?
        } else {
            self.db.list_users(offset as _, page_size as _).await?
        };

        let users_info: Vec<AdminUserInfo> =
            users.iter().map(Self::map_user_to_admin_user).collect();

        Ok(AdminUsersResponse {
            data: users_info,
            page,
            limit: page_size,
            total: total as u32,
        })
    }

    async fn update_user(
        &self,
        auth: Nip98Auth,
        uid: u64,
        req: AdminUserRequest,
    ) -> Result<AdminUserInfo> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;

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
                // Log admin action with stopped streams information
                let action = "block_user";
                let message = format!("User {} blocked", uid);
                let metadata = serde_json::json!({
                    "target_user_id": uid,
                    "blocked_status": true
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
            && credit_amount != 0
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

        let u = self.db.get_user(uid).await?;
        Ok(Self::map_user_to_admin_user(&u))
    }

    async fn get_user_balance_history(
        &self,
        auth: Nip98Auth,
        uid: u64,
        page: u32,
        page_size: u32,
    ) -> Result<HistoryResponse> {
        self.check_admin_access(&auth.pubkey).await?;
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

    async fn get_user_streams(
        &self,
        auth: Nip98Auth,
        uid: u64,
        page: u32,
        page_size: u32,
    ) -> Result<AdminUserStreamsResponse> {
        self.check_admin_access(&auth.pubkey).await?;
        let offset = page * page_size;
        let (streams, total) = self
            .db
            .get_user_streams(uid, offset as _, page_size as _)
            .await?;

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
            data: streams_info,
            page,
            limit: page_size,
            total: total as u32,
        })
    }

    async fn get_user_stream_key(
        &self,
        auth: Nip98Auth,
        uid: u64,
    ) -> Result<AdminStreamKeyResponse> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;
        let user = self.db.get_user(uid).await?;

        // Log the admin action
        self.db
            .log_admin_action(
                admin_uid,
                "view_stream_key",
                Some("user"),
                Some(&uid.to_string()),
                &format!("Admin viewed stream key for user {}", uid),
                Some(&format!(r#"{{"target_user_id": {}}}"#, uid)),
            )
            .await?;

        Ok(AdminStreamKeyResponse {
            stream_key: user.stream_key,
        })
    }

    async fn regenerate_user_stream_key(
        &self,
        auth: Nip98Auth,
        uid: u64,
    ) -> Result<AdminStreamKeyResponse> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;
        // Generate a new UUID for the stream key
        let new_key = Uuid::new_v4().to_string();

        // Update the user's main stream key
        self.db.update_user_stream_key(uid, &new_key).await?;

        // Log admin action
        let message = format!("Regenerated stream key for user {}", uid);
        let metadata = serde_json::json!({
            "target_user_id": uid,
            "new_key": new_key
        });
        self.db
            .log_admin_action(
                admin_uid,
                "regenerate_stream_key",
                Some("user"),
                Some(&uid.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(AdminStreamKeyResponse {
            stream_key: new_key,
        })
    }

    async fn get_audit_log(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> Result<AdminAuditLogResponse> {
        self.check_admin_access(&auth.pubkey).await?;
        let offset = page * page_size;
        let (logs, total) = self
            .db
            .get_audit_logs_with_pubkeys(offset as _, page_size as _)
            .await?;

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
            data: logs_info,
            page,
            limit: page_size,
            total: total as u32,
        })
    }

    async fn get_ingest_endpoints(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> Result<AdminIngestEndpointsResponse> {
        self.check_admin_access(&auth.pubkey).await?;
        let offset = page * page_size;
        // TODO: implement db pagination
        let endpoints = self.db.get_ingest_endpoints().await?;
        let total = endpoints.len() as u64;

        let paginated_endpoints = endpoints
            .into_iter()
            .skip(offset as usize)
            .take(page_size as usize)
            .map(|endpoint| self.map_endpoint_to_admin_endpoint(endpoint))
            .collect();

        Ok(AdminIngestEndpointsResponse {
            data: paginated_endpoints,
            page,
            limit: page_size,
            total: total as u32,
        })
    }

    async fn create_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        req: AdminIngestEndpointRequest,
    ) -> Result<AdminIngestEndpointResponse> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;

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

    async fn update_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        id: u64,
        req: AdminIngestEndpointRequest,
    ) -> Result<AdminIngestEndpointResponse> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;
        let capabilities_str = req.capabilities.as_ref().map(|caps| caps.join(","));
        self.db
            .update_ingest_endpoint(id, &req.name, req.cost, capabilities_str.as_deref())
            .await?;

        // Log admin action
        let message = format!(
            "Updated ingest endpoint {}: {} (cost: {})",
            id, req.name, req.cost
        );
        let metadata = serde_json::json!({
            "endpoint_id": id,
            "name": req.name,
            "cost": req.cost,
            "capabilities": req.capabilities
        });
        self.db
            .log_admin_action(
                admin_uid,
                "update_ingest_endpoint",
                Some("ingest_endpoint"),
                Some(&id.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(AdminIngestEndpointResponse {
            id,
            name: req.name.clone(),
            cost: req.cost,
            capabilities: req.capabilities,
            urls: self.generate_endpoint_urls(&req.name),
        })
    }

    async fn get_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        id: u64,
    ) -> Result<AdminIngestEndpointResponse> {
        self.check_admin_access(&auth.pubkey).await?;
        let endpoint = self.db.get_ingest_endpoint(id).await?;
        Ok(self.map_endpoint_to_admin_endpoint(endpoint))
    }

    async fn delete_ingest_endpoint(&self, auth: Nip98Auth, id: u64) -> Result<()> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;
        // Get the endpoint first for logging
        let endpoint = self.db.get_ingest_endpoint(id).await?;

        // Delete the endpoint
        self.db.delete_ingest_endpoint(id).await?;

        // Log admin action
        let message = format!("Deleted ingest endpoint {}: {}", id, endpoint.name);
        let metadata = serde_json::json!({
            "endpoint_id": id,
            "name": endpoint.name,
            "cost": endpoint.cost,
            "capabilities": endpoint.capabilities
        });
        self.db
            .log_admin_action(
                admin_uid,
                "delete_ingest_endpoint",
                Some("ingest_endpoint"),
                Some(&id.to_string()),
                &message,
                Some(&metadata.to_string()),
            )
            .await?;

        Ok(())
    }

    async fn get_stream_logs(&self, auth: Nip98Auth, stream: Uuid) -> Result<Option<String>> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;

        // Construct path to pipeline.log in stream's output directory
        // Using the parsed UUID's string representation ensures it's sanitized
        let log_path = self
            .output_dir
            .join(stream.to_string())
            .join("pipeline.log");

        // Try to read the log file
        let log_content = match tokio::fs::read_to_string(&log_path).await {
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
                Some(&stream.to_string()),
                &format!("Admin viewed pipeline log for stream {}", stream),
                None,
            )
            .await?;

        Ok(Some(log_content))
    }

    async fn get_payments(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
        user_id: Option<u64>,
        payment_type: Option<String>,
        is_paid: Option<bool>,
    ) -> Result<AdminPaymentsResponse> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;
        
        // Parse payment type
        let payment_type_enum = payment_type.as_ref().and_then(|pt| {
            match pt.to_lowercase().as_str() {
                "topup" | "top_up" => Some(zap_stream_db::PaymentType::TopUp),
                "zap" => Some(zap_stream_db::PaymentType::Zap),
                "credit" => Some(zap_stream_db::PaymentType::Credit),
                "withdrawal" => Some(zap_stream_db::PaymentType::Withdrawal),
                "admissionfee" | "admission_fee" => Some(zap_stream_db::PaymentType::AdmissionFee),
                _ => None,
            }
        });
        
        let offset = (page * page_size) as u64;
        let limit = page_size as u64;
        
        let payments = self
            .db
            .get_all_payments(offset, limit, user_id, payment_type_enum, is_paid)
            .await?;
        
        let total = self
            .db
            .count_all_payments(user_id, payment_type_enum, is_paid)
            .await?;
        
        // Get user pubkeys for the payments
        let mut payments_info = Vec::new();
        for payment in payments {
            let user = self.db.get_user(payment.user_id).await.ok();
            let payment_type_str = match payment.payment_type {
                zap_stream_db::PaymentType::TopUp => "TopUp",
                zap_stream_db::PaymentType::Zap => "Zap",
                zap_stream_db::PaymentType::Credit => "Credit",
                zap_stream_db::PaymentType::Withdrawal => "Withdrawal",
                zap_stream_db::PaymentType::AdmissionFee => "AdmissionFee",
            };
            
            payments_info.push(AdminPaymentInfo {
                payment_hash: hex::encode(&payment.payment_hash),
                user_id: payment.user_id,
                user_pubkey: user.map(|u| hex::encode(&u.pubkey)),
                amount: payment.amount,
                is_paid: payment.is_paid,
                payment_type: payment_type_str.to_string(),
                fee: payment.fee,
                created: payment.created.timestamp() as u64,
                expires: payment.expires.timestamp() as u64,
            });
        }
        
        // Log admin action
        self.db
            .log_admin_action(
                admin_uid,
                "list_payments",
                Some("payment"),
                None,
                &format!("Admin listed payments (page: {}, limit: {})", page, page_size),
                Some(&format!(
                    r#"{{"page": {}, "limit": {}, "user_id": {:?}, "payment_type": {:?}, "is_paid": {:?}}}"#,
                    page, page_size, user_id, payment_type, is_paid
                )),
            )
            .await?;
        
        Ok(AdminPaymentsResponse {
            data: payments_info,
            page,
            limit: page_size,
            total,
        })
    }

    async fn get_payments_summary(&self, auth: Nip98Auth) -> Result<AdminPaymentsSummary> {
        let admin_uid = self.check_admin_access(&auth.pubkey).await?;
        
        // Get total users and balance
        let total_users = self.db.get_total_user_count().await?;
        let total_balance = self.db.get_total_balance().await?;
        
        // Get total stream costs
        let total_stream_costs = self.db.get_total_stream_costs().await?;
        
        // Calculate balance difference (total balance - total stream costs)
        let balance_difference = total_balance - (total_stream_costs as i64);
        
        // Get payment statistics by type
        let (topup_count, topup_amount, topup_paid_count, topup_paid_amount) = 
            self.db.get_payment_stats_by_type(zap_stream_db::PaymentType::TopUp).await?;
        let (zap_count, zap_amount, zap_paid_count, zap_paid_amount) = 
            self.db.get_payment_stats_by_type(zap_stream_db::PaymentType::Zap).await?;
        let (credit_count, credit_amount, credit_paid_count, credit_paid_amount) = 
            self.db.get_payment_stats_by_type(zap_stream_db::PaymentType::Credit).await?;
        let (withdrawal_count, withdrawal_amount, withdrawal_paid_count, withdrawal_paid_amount) = 
            self.db.get_payment_stats_by_type(zap_stream_db::PaymentType::Withdrawal).await?;
        let (admission_count, admission_amount, admission_paid_count, admission_paid_amount) = 
            self.db.get_payment_stats_by_type(zap_stream_db::PaymentType::AdmissionFee).await?;
        
        // Calculate totals
        let total_payments = topup_count + zap_count + credit_count + withdrawal_count + admission_count;
        let total_paid_amount = topup_paid_amount + zap_paid_amount + credit_paid_amount + withdrawal_paid_amount + admission_paid_amount;
        let total_pending_amount = (topup_amount - topup_paid_amount) + 
                                    (zap_amount - zap_paid_amount) + 
                                    (credit_amount - credit_paid_amount) + 
                                    (withdrawal_amount - withdrawal_paid_amount) + 
                                    (admission_amount - admission_paid_amount);
        
        // Log admin action
        self.db
            .log_admin_action(
                admin_uid,
                "view_payments_summary",
                Some("payment"),
                None,
                "Admin viewed payments summary",
                None,
            )
            .await?;
        
        Ok(AdminPaymentsSummary {
            total_users,
            total_balance,
            total_stream_costs,
            balance_difference,
            total_payments,
            total_paid_amount,
            total_pending_amount,
            payments_by_type: AdminPaymentsByType {
                top_up: AdminPaymentTypeStats {
                    count: topup_count,
                    total_amount: topup_amount,
                    paid_count: topup_paid_count,
                    paid_amount: topup_paid_amount,
                },
                zap: AdminPaymentTypeStats {
                    count: zap_count,
                    total_amount: zap_amount,
                    paid_count: zap_paid_count,
                    paid_amount: zap_paid_amount,
                },
                credit: AdminPaymentTypeStats {
                    count: credit_count,
                    total_amount: credit_amount,
                    paid_count: credit_paid_count,
                    paid_amount: credit_paid_amount,
                },
                withdrawal: AdminPaymentTypeStats {
                    count: withdrawal_count,
                    total_amount: withdrawal_amount,
                    paid_count: withdrawal_paid_count,
                    paid_amount: withdrawal_paid_amount,
                },
                admission_fee: AdminPaymentTypeStats {
                    count: admission_count,
                    total_amount: admission_amount,
                    paid_count: admission_paid_count,
                    paid_amount: admission_paid_amount,
                },
            },
        })
    }
}
