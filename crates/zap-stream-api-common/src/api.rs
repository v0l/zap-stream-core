use crate::{
    AccountInfo, CreateStreamKeyRequest, CreateStreamKeyResponse, ForwardRequest, ForwardResponse,
    GameInfo, HistoryResponse, Nip98Auth, PatchAccount, PatchEvent, StreamKey, TopupResponse,
    UpdateForwardRequest,
};
use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait ZapStreamApi: Clone + Send + Sync {
    async fn get_account(&self, auth: Nip98Auth) -> Result<AccountInfo>;
    async fn update_account(&self, auth: Nip98Auth, patch_account: PatchAccount) -> Result<()>;
    async fn update_event(&self, auth: Nip98Auth, patch: PatchEvent) -> Result<()>;
    async fn delete_event(&self, auth: Nip98Auth, stream_id: Uuid) -> Result<()>;
    async fn create_forward(&self, auth: Nip98Auth, req: ForwardRequest)
    -> Result<ForwardResponse>;
    async fn delete_forward(&self, auth: Nip98Auth, forward_id: u64) -> Result<()>;
    async fn update_forward(
        &self,
        auth: Nip98Auth,
        id: u64,
        req: UpdateForwardRequest,
    ) -> Result<ForwardResponse>;
    async fn get_balance_history(
        &self,
        auth: Nip98Auth,
        page: u32,
        page_size: u32,
    ) -> Result<HistoryResponse>;
    async fn get_stream_keys(&self, auth: Nip98Auth) -> Result<Vec<StreamKey>>;
    async fn create_stream_key(
        &self,
        auth: Nip98Auth,
        req: CreateStreamKeyRequest,
    ) -> Result<CreateStreamKeyResponse>;
    async fn delete_stream_key(&self, auth: Nip98Auth, key_id: u64) -> Result<()>;
    async fn topup(
        &self,
        pubkey: [u8; 32],
        amount: u64,
        zap: Option<String>,
    ) -> Result<TopupResponse>;
    //async fn withdraw();
    async fn search_games(&self, q: String) -> Result<Vec<GameInfo>>;
    async fn get_game(&self, id: String) -> Result<GameInfo>;
}
