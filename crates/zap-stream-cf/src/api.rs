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
    async fn get_account(&self, _auth: Nip98Auth) -> anyhow::Result<AccountInfo> {
        todo!()
    }

    async fn update_account(
        &self,
        _auth: Nip98Auth,
        _patch_account: PatchAccount,
    ) -> anyhow::Result<()> {
        todo!()
    }

    async fn update_event(&self, _auth: Nip98Auth, _patch: PatchEvent) -> anyhow::Result<()> {
        todo!()
    }

    async fn delete_event(&self, _auth: Nip98Auth, _stream_id: Uuid) -> anyhow::Result<()> {
        todo!()
    }

    async fn create_forward(
        &self,
        _auth: Nip98Auth,
        _req: ForwardRequest,
    ) -> anyhow::Result<ForwardResponse> {
        todo!()
    }

    async fn delete_forward(&self, _auth: Nip98Auth, _forward_id: u64) -> anyhow::Result<()> {
        todo!()
    }

    async fn update_forward(
        &self,
        _auth: Nip98Auth,
        _id: u64,
        _req: UpdateForwardRequest,
    ) -> anyhow::Result<ForwardResponse> {
        todo!()
    }

    async fn get_balance_history(
        &self,
        _auth: Nip98Auth,
        _page: u32,
        _page_size: u32,
    ) -> anyhow::Result<HistoryResponse> {
        todo!()
    }

    async fn get_stream_keys(&self, _auth: Nip98Auth) -> anyhow::Result<Vec<StreamKey>> {
        todo!()
    }

    async fn create_stream_key(
        &self,
        _auth: Nip98Auth,
        _req: CreateStreamKeyRequest,
    ) -> anyhow::Result<CreateStreamKeyResponse> {
        todo!()
    }

    async fn delete_stream_key(&self, _auth: Nip98Auth, _key_id: u64) -> anyhow::Result<()> {
        todo!()
    }

    async fn topup(
        &self,
        _pubkey: [u8; 32],
        _amount: u64,
        _zap: Option<String>,
    ) -> anyhow::Result<TopupResponse> {
        todo!()
    }

    async fn search_games(&self, _q: String) -> anyhow::Result<Vec<GameInfo>> {
        todo!()
    }

    async fn get_game(&self, _id: String) -> anyhow::Result<GameInfo> {
        todo!()
    }
}

#[async_trait]
impl ZapStreamAdminApi for CfApiWrapper {
    async fn get_users(
        &self,
        _auth: Nip98Auth,
        _page: u32,
        _page_size: u32,
        _search: Option<String>,
    ) -> anyhow::Result<AdminUsersResponse> {
        todo!()
    }

    async fn update_user(
        &self,
        _auth: Nip98Auth,
        _uid: u64,
        _req: AdminUserRequest,
    ) -> anyhow::Result<AdminUserInfo> {
        todo!()
    }

    async fn get_user_balance_history(
        &self,
        _auth: Nip98Auth,
        _uid: u64,
        _page: u32,
        _page_size: u32,
    ) -> anyhow::Result<HistoryResponse> {
        todo!()
    }

    async fn get_user_streams(
        &self,
        _auth: Nip98Auth,
        _uid: u64,
        _page: u32,
        _page_size: u32,
    ) -> anyhow::Result<AdminUserStreamsResponse> {
        todo!()
    }

    async fn get_user_stream_key(
        &self,
        _auth: Nip98Auth,
        _uid: u64,
    ) -> anyhow::Result<AdminStreamKeyResponse> {
        todo!()
    }

    async fn regenerate_user_stream_key(
        &self,
        _auth: Nip98Auth,
        _uid: u64,
    ) -> anyhow::Result<AdminStreamKeyResponse> {
        todo!()
    }

    async fn get_audit_log(
        &self,
        _auth: Nip98Auth,
        _page: u32,
        _page_size: u32,
    ) -> anyhow::Result<AdminAuditLogResponse> {
        todo!()
    }

    async fn get_ingest_endpoints(
        &self,
        _auth: Nip98Auth,
        _page: u32,
        _page_size: u32,
    ) -> anyhow::Result<AdminIngestEndpointsResponse> {
        todo!()
    }

    async fn create_ingest_endpoint(
        &self,
        _auth: Nip98Auth,
        _req: AdminIngestEndpointRequest,
    ) -> anyhow::Result<AdminIngestEndpointResponse> {
        todo!()
    }

    async fn update_ingest_endpoint(
        &self,
        _auth: Nip98Auth,
        _id: u64,
        _req: AdminIngestEndpointRequest,
    ) -> anyhow::Result<AdminIngestEndpointResponse> {
        todo!()
    }

    async fn get_ingest_endpoint(
        &self,
        _auth: Nip98Auth,
        _id: u64,
    ) -> anyhow::Result<AdminIngestEndpointResponse> {
        todo!()
    }

    async fn delete_ingest_endpoint(&self, _auth: Nip98Auth, _id: u64) -> anyhow::Result<()> {
        todo!()
    }

    async fn get_stream_logs(
        &self,
        _auth: Nip98Auth,
        _stream: Uuid,
    ) -> anyhow::Result<Option<String>> {
        todo!()
    }
}
