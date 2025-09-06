use anyhow::{Result, bail};
use base64::Engine;
use chrono::{DateTime, Utc};
use nostr_sdk::{Alphabet, Event, Kind, PublicKey, SingleLetterTag, TagKind, serde_json};
use url::Url;
use zap_stream_db::ZapStreamDb;

#[derive(Debug, Clone)]
pub struct AuthResult {
    pub pubkey: PublicKey,
    pub event: Event,
    pub user_id: u64,
    pub is_admin: bool,
}

pub enum TokenSource {
    /// HTTP Authorization header: "Nostr <base64_token>"
    HttpHeader(String),
    /// WebSocket direct base64 token
    WebSocketToken(String),
}

pub struct AuthRequest {
    pub token_source: TokenSource,
    pub expected_url: Url,
    pub expected_method: String,
    pub skip_url_check: bool,
}

/// Generic NIP-98 authentication that works for both HTTP and WebSocket
pub async fn authenticate_nip98(auth_request: AuthRequest, db: &ZapStreamDb) -> Result<AuthResult> {
    // Extract the base64 token based on source
    let token = match &auth_request.token_source {
        TokenSource::HttpHeader(auth_header) => {
            if !auth_header.starts_with("Nostr ") {
                bail!("Invalid authorization scheme");
            }
            &auth_header[6..]
        }
        TokenSource::WebSocketToken(token) => token.as_str(),
    };

    // Decode the base64 token
    let decoded = base64::engine::general_purpose::STANDARD.decode(token.as_bytes())?;

    // Check if decoded data starts with '{'
    if decoded.is_empty() || decoded[0] != b'{' {
        bail!("Invalid token");
    }

    let json = String::from_utf8(decoded)?;
    let event: Event = serde_json::from_str(&json)?;

    // Verify signature
    if event.verify().is_err() {
        bail!("Invalid nostr event, invalid signature");
    }

    // Check event kind (NIP-98: HTTP Auth, kind 27235)
    if event.kind != Kind::Custom(27235) {
        bail!("Invalid nostr event, wrong kind");
    }

    // Check timestamp (within 120 seconds)
    let now = Utc::now();
    let event_time = DateTime::from_timestamp(event.created_at.as_u64() as i64, 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
    let diff_seconds = (now - event_time).num_seconds().abs();
    if diff_seconds > 120 {
        bail!("Invalid nostr event, timestamp out of range");
    }

    // Check URL tag
    let url_tag: Url = event
        .tags
        .iter()
        .find(|tag| tag.kind() == TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::U)))
        .and_then(|tag| tag.content())
        .ok_or_else(|| anyhow::anyhow!("Missing URL tag"))?
        .parse()?;

    if auth_request.expected_url.as_str() != url_tag.as_str() && !auth_request.skip_url_check {
        bail!(
            "Invalid nostr event, URL tag invalid. Expected: {}, Got: {}",
            auth_request.expected_url,
            url_tag
        );
    }

    // Check method tag
    let method_tag = event
        .tags
        .iter()
        .find(|tag| tag.kind() == TagKind::Method)
        .and_then(|tag| tag.content())
        .ok_or_else(|| anyhow::anyhow!("Missing method tag"))?;

    if !method_tag.eq_ignore_ascii_case(&auth_request.expected_method) {
        bail!("Invalid nostr event, method tag invalid");
    }

    // Get user ID and check admin status
    let user_id = db.upsert_user(&event.pubkey.to_bytes()).await?;
    let is_admin = db.is_admin(user_id).await?;

    Ok(AuthResult {
        pubkey: event.pubkey,
        event,
        user_id,
        is_admin,
    })
}
