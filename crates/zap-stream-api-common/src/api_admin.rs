use crate::{AdminAuditLogResponse, AdminIngestEndpointRequest, AdminIngestEndpointResponse, AdminIngestEndpointsResponse, AdminPaymentsResponse, AdminPaymentsSummary, AdminStreamKeyResponse, AdminUserInfo, AdminUserRequest, AdminUserStreamsResponse, AdminUsersResponse, HistoryResponse, Nip98Auth};
use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait ZapStreamAdminApi: Clone + Send + Sync {
    async fn get_users(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
        search: Option<String>,
    ) -> Result<AdminUsersResponse>;

    async fn update_user(&self, auth: Nip98Auth, uid: u64, req: AdminUserRequest) -> Result<AdminUserInfo>;

    async fn get_user_balance_history(
        &self,
        auth: Nip98Auth,
        uid: u64,
        page: u32,
        page_size: u32,
    ) -> Result<HistoryResponse>;

    async fn get_user_streams(
        &self,
        auth: Nip98Auth,
        uid: u64,
        page: u32,
        page_size: u32,
    ) -> Result<AdminUserStreamsResponse>;

    async fn get_user_stream_key(
        &self,
        auth: Nip98Auth,
        uid: u64,
    ) -> Result<AdminStreamKeyResponse>;

    async fn regenerate_user_stream_key(
        &self,
        auth: Nip98Auth,
        uid: u64,
    ) -> Result<AdminStreamKeyResponse>;

    async fn get_audit_log(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> Result<AdminAuditLogResponse>;

    async fn get_ingest_endpoints(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> Result<AdminIngestEndpointsResponse>;

    async fn create_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        req: AdminIngestEndpointRequest,
    ) -> Result<AdminIngestEndpointResponse>;

    async fn update_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        id: u64,
        req: AdminIngestEndpointRequest,
    ) -> Result<AdminIngestEndpointResponse>;

    async fn get_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        id: u64,
    ) -> Result<AdminIngestEndpointResponse>;

    async fn delete_ingest_endpoint(&self, auth: Nip98Auth, id: u64) -> Result<()>;

    async fn get_stream_logs(&self, auth: Nip98Auth, stream: Uuid) -> Result<Option<String>>;

    async fn get_payments(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
        user_id: Option<u64>,
        payment_type: Option<String>,
        is_paid: Option<bool>,
    ) -> Result<AdminPaymentsResponse>;

    async fn get_payments_summary(&self, auth: Nip98Auth) -> Result<AdminPaymentsSummary>;
}
