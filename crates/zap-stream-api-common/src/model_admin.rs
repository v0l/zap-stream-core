use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct AdminUserInfo {
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
pub struct AdminUserRequest {
    pub set_admin: Option<bool>,
    pub set_blocked: Option<bool>,
    pub set_stream_dump_recording: Option<bool>,
    pub add_credit: Option<i64>,
    pub memo: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub tags: Option<Vec<String>>,
    pub content_warning: Option<String>,
    pub goal: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct AdminStreamInfo {
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
pub struct AdminStreamKeyResponse {
    pub stream_key: String,
}

#[derive(Deserialize, Serialize)]
pub struct AdminAuditLogEntry {
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
pub struct AdminIngestEndpointRequest {
    pub name: String,
    pub cost: u64,
    pub capabilities: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize)]
pub struct AdminIngestEndpointResponse {
    pub id: u64,
    pub name: String,
    pub cost: u64,
    pub capabilities: Option<Vec<String>>,
    pub urls: Vec<String>,
}

pub type AdminIngestEndpointsResponse = AdminPageResponse<AdminIngestEndpointResponse>;
pub type AdminAuditLogResponse = AdminPageResponse<AdminAuditLogEntry>;
pub type AdminUserStreamsResponse = AdminPageResponse<AdminStreamInfo>;
pub type AdminUsersResponse = AdminPageResponse<AdminUserInfo>;
pub type AdminPaymentsResponse = AdminPageResponse<AdminPaymentInfo>;

#[derive(Deserialize, Serialize)]
pub struct AdminPageResponse<T> {
    pub data: Vec<T>,
    pub page: u32,
    pub limit: u32,
    pub total: u32,
}

#[derive(Deserialize, Serialize)]
pub struct AdminPaymentInfo {
    pub payment_hash: String,
    pub user_id: u64,
    pub user_pubkey: Option<String>,
    pub amount: i64,
    pub is_paid: bool,
    pub payment_type: String,
    pub fee: u64,
    pub created: u64,
    pub expires: u64,
}

#[derive(Deserialize, Serialize)]
pub struct AdminPaymentsSummary {
    pub total_users: u32,
    pub total_balance: i64,
    pub total_stream_costs: u64,
    pub payments_by_type: AdminPaymentsByType,
}

#[derive(Deserialize, Serialize)]
pub struct AdminPaymentsByType {
    pub top_up: AdminPaymentTypeStats,
    pub zap: AdminPaymentTypeStats,
    pub credit: AdminPaymentTypeStats,
    pub withdrawal: AdminPaymentTypeStats,
    pub admission_fee: AdminPaymentTypeStats,
}

#[derive(Deserialize, Serialize)]
pub struct AdminPaymentTypeStats {
    pub count: u32,
    pub total_amount: i64,
    pub paid_count: u32,
    pub paid_amount: i64,
}

#[derive(Deserialize, Serialize)]
pub struct AdminBalanceOffsetInfo {
    pub user_id: u64,
    pub pubkey: String,
    pub current_balance: i64,
    pub total_payments: i64,
    pub total_stream_costs: i64,
    pub balance_offset: i64,
}

pub type AdminBalanceOffsetsResponse = AdminPageResponse<AdminBalanceOffsetInfo>;
