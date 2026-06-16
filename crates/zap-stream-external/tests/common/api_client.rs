use base64::Engine;
use nostr_sdk::{Client, EventBuilder, JsonUtil, Keys, Kind, Tag};
use reqwest::StatusCode;
use serde_json::Value;

pub struct ApiClient {
    http: reqwest::Client,
    nostr: Client,
    keys: Keys,
    base_url: String,
}

impl ApiClient {
    pub async fn new(nsec: &str, base_url: &str) -> Self {
        let keys = Keys::parse(nsec).expect("valid nsec");
        let nostr = Client::builder().signer(keys.clone()).build();
        Self {
            http: reqwest::Client::new(),
            nostr,
            keys,
            base_url: base_url.to_string(),
        }
    }

    /// Returns the hex-encoded public key for this test user.
    pub fn pubkey_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    /// Build a NIP-98 auth token (base64-encoded signed kind 27235 event).
    async fn make_nip98_token(&self, url: &str, method: &str) -> String {
        let eb = EventBuilder::new(Kind::Custom(27235), "").tags([
            Tag::parse(["u", url]).expect("valid u tag"),
            Tag::parse(["method", method]).expect("valid method tag"),
        ]);
        let event = self
            .nostr
            .sign_event_builder(eb)
            .await
            .expect("signing failed");
        let json = event.as_json();
        base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
    }

    /// GET /api/v1/account with NIP-98 auth.
    pub async fn get_account(&self) -> Value {
        let url = format!("{}/account", self.base_url);
        let token = self.make_nip98_token(&url, "GET").await;
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Nostr {}", token))
            .send()
            .await
            .expect("GET /account failed");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET /account returned {}",
            resp.status()
        );
        resp.json::<Value>().await.expect("invalid JSON response")
    }

    /// POST /api/v1/keys to create a custom stream key.
    pub async fn create_key(&self, title: &str, summary: &str, tags: &[&str]) -> Value {
        let url = format!("{}/keys", self.base_url);
        let token = self.make_nip98_token(&url, "POST").await;
        let body = serde_json::json!({
            "event": {
                "title": title,
                "summary": summary,
                "tags": tags,
            }
        });
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Nostr {}", token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .expect("POST /keys failed");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "POST /keys returned {}",
            resp.status()
        );
        resp.json::<Value>().await.expect("invalid JSON response")
    }

    /// GET /api/v1/keys to list all stream keys.
    pub async fn list_keys(&self) -> Value {
        let url = format!("{}/keys", self.base_url);
        let token = self.make_nip98_token(&url, "GET").await;
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Nostr {}", token))
            .send()
            .await
            .expect("GET /keys failed");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET /keys returned {}",
            resp.status()
        );
        resp.json::<Value>().await.expect("invalid JSON response")
    }
}
