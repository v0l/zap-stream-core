use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use async_trait::async_trait;
use base64::Engine;
use nostr_sdk::{EventBuilder, Keys, Kind, Tag, JsonUtil};
use payments_rs::lightning::{AddInvoiceRequest, AddInvoiceResponse, InvoiceUpdate, LightningNode};
use std::pin::Pin;
use std::sync::Arc;
use zap_stream_db::ZapStreamDb;

#[path = "../../zap-stream-db/tests/common/mod.rs"]
mod db_container;

pub use db_container::TestDb;

struct NoLightning;

#[async_trait]
impl LightningNode for NoLightning {
    async fn add_invoice(&self, _req: AddInvoiceRequest) -> anyhow::Result<AddInvoiceResponse> {
        anyhow::bail!("no lightning in tests")
    }

    async fn cancel_invoice(&self, _id: &Vec<u8>) -> anyhow::Result<()> {
        Ok(())
    }

    async fn subscribe_invoices(
        &self,
        _from_payment_hash: Option<Vec<u8>>,
    ) -> anyhow::Result<Pin<Box<dyn futures_util::Stream<Item = InvoiceUpdate> + Send>>> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

pub fn test_settings() -> Settings {
    serde_json::from_value(serde_json::json!({
        "endpoints": ["rtmp://127.0.0.1:1935"],
        "endpoints_public_hostname": "localhost",
        "output_dir": std::env::temp_dir().to_string_lossy(),
        "public_url": "http://localhost",
        "listen_http": "127.0.0.1:0",
        "overseer": {
            "database": "unused",
            "payments": {"lnurl": {"address": "a@example.com"}},
            "relays": [],
            "nsec": Keys::generate().secret_key().to_secret_hex(),
        }
    }))
    .unwrap()
}

pub fn test_overseer(db: &ZapStreamDb) -> Arc<ZapStreamOverseer> {
    Arc::new(ZapStreamOverseer::for_tests(
        test_settings(),
        db.clone(),
        Arc::new(NoLightning),
    ))
}

pub fn nip98_header(keys: &Keys, method: &str, url: &str) -> String {
    let ev = EventBuilder::new(Kind::Custom(27235), "")
        .tag(Tag::parse(["u", url]).unwrap())
        .tag(Tag::parse(["method", method]).unwrap())
        .sign_with_keys(keys)
        .unwrap();
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(ev.as_json())
    )
}
