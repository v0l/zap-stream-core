use async_trait::async_trait;
use uuid::Uuid;
use zap_stream_api_common::{
    AccountInfo, AdminAuditLogResponse, AdminIngestEndpointRequest, AdminIngestEndpointResponse,
    AdminIngestEndpointsResponse, AdminStreamKeyResponse, AdminUserInfo, AdminUserRequest,
    AdminUserStreamsResponse, AdminUsersResponse, CreateStreamKeyRequest, CreateStreamKeyResponse,
    ForwardRequest, ForwardResponse, GameInfo, HistoryResponse, Nip98Auth, PatchAccount,
    PatchEvent, StreamKey, TopupResponse, UpdateForwardRequest, ZapStreamAdminApi, ZapStreamApi,
};

#[derive(Clone)]
pub struct CfApiWrapper {}

impl CfApiWrapper {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ZapStreamApi for CfApiWrapper {
    async fn get_account(&self, auth: Nip98Auth) -> anyhow::Result<AccountInfo> {
        todo!()
    }

    async fn update_account(
        &self,
        auth: Nip98Auth,
        patch_account: PatchAccount,
    ) -> anyhow::Result<()> {
        todo!()
    }

    async fn update_event(&self, auth: Nip98Auth, patch: PatchEvent) -> anyhow::Result<()> {
        todo!()
    }

    async fn delete_event(&self, auth: Nip98Auth, stream_id: Uuid) -> anyhow::Result<()> {
        todo!()
    }

    async fn create_forward(
        &self,
        auth: Nip98Auth,
        req: ForwardRequest,
    ) -> anyhow::Result<ForwardResponse> {
        todo!()
    }

    async fn delete_forward(&self, auth: Nip98Auth, forward_id: u64) -> anyhow::Result<()> {
        todo!()
    }

    async fn update_forward(
        &self,
        auth: Nip98Auth,
        id: u64,
        req: UpdateForwardRequest,
    ) -> anyhow::Result<ForwardResponse> {
        todo!()
    }

    async fn get_balance_history(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> anyhow::Result<HistoryResponse> {
        todo!()
    }

    async fn get_stream_keys(&self, auth: Nip98Auth) -> anyhow::Result<Vec<StreamKey>> {
        todo!()
    }

    async fn create_stream_key(
        &self,
        auth: Nip98Auth,
        req: CreateStreamKeyRequest,
    ) -> anyhow::Result<CreateStreamKeyResponse> {
        todo!()
    }

    async fn delete_stream_key(&self, auth: Nip98Auth, key_id: u64) -> anyhow::Result<()> {
        todo!()
    }

    async fn topup(
        &self,
        pubkey: [u8; 32],
        amount: u64,
        zap: Option<String>,
    ) -> anyhow::Result<TopupResponse> {
        todo!()
    }

    async fn search_games(&self, q: String) -> anyhow::Result<Vec<GameInfo>> {
        todo!()
    }

    async fn get_game(&self, id: String) -> anyhow::Result<GameInfo> {
        todo!()
    }
}

#[async_trait]
impl ZapStreamAdminApi for CfApiWrapper {
    async fn get_users(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
        search: Option<String>,
    ) -> anyhow::Result<AdminUsersResponse> {
        todo!()
    }

    async fn update_user(
        &self,
        auth: Nip98Auth,
        uid: u64,
        req: AdminUserRequest,
    ) -> anyhow::Result<AdminUserInfo> {
        todo!()
    }

    async fn get_user_balance_history(
        &self,
        auth: Nip98Auth,
        uid: u64,
        page: u32,
        page_size: u32,
    ) -> anyhow::Result<HistoryResponse> {
        todo!()
    }

    async fn get_user_streams(
        &self,
        auth: Nip98Auth,
        uid: u64,
        page: u32,
        page_size: u32,
    ) -> anyhow::Result<AdminUserStreamsResponse> {
        todo!()
    }

    async fn get_user_stream_key(
        &self,
        auth: Nip98Auth,
        uid: u64,
    ) -> anyhow::Result<AdminStreamKeyResponse> {
        todo!()
    }

    async fn regenerate_user_stream_key(
        &self,
        auth: Nip98Auth,
        uid: u64,
    ) -> anyhow::Result<AdminStreamKeyResponse> {
        todo!()
    }

    async fn get_audit_log(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> anyhow::Result<AdminAuditLogResponse> {
        todo!()
    }

    async fn get_ingest_endpoints(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> anyhow::Result<AdminIngestEndpointsResponse> {
        todo!()
    }

    async fn create_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        req: AdminIngestEndpointRequest,
    ) -> anyhow::Result<AdminIngestEndpointResponse> {
        todo!()
    }

    async fn update_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        id: u64,
        req: AdminIngestEndpointRequest,
    ) -> anyhow::Result<AdminIngestEndpointResponse> {
        todo!()
    }

    async fn get_ingest_endpoint(
        &self,
        auth: Nip98Auth,
        id: u64,
    ) -> anyhow::Result<AdminIngestEndpointResponse> {
        todo!()
    }

    async fn delete_ingest_endpoint(&self, auth: Nip98Auth, id: u64) -> anyhow::Result<()> {
        todo!()
    }

    async fn get_stream_logs(
        &self,
        auth: Nip98Auth,
        stream: Uuid,
    ) -> anyhow::Result<Option<String>> {
        todo!()
    }
}
