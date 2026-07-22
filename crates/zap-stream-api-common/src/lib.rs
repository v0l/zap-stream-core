use anyhow::{Result, anyhow, bail};
use base64::Engine;
use chrono::{DateTime, Utc};
use nostr_sdk::{Alphabet, Event, JsonUtil, Kind, SingleLetterTag, TagKind};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

mod api;

pub use api::*;
mod model;
pub use model::*;

#[cfg(feature = "admin")]
mod api_admin;
#[cfg(feature = "admin")]
pub use api_admin::*;
#[cfg(feature = "admin")]
mod model_admin;
#[cfg(feature = "admin")]
pub use model_admin::*;
mod game_db;
pub use game_db::*;
#[cfg(feature = "axum")]
mod api_axum;
#[cfg(feature = "axum")]
pub use api_axum::*;
#[cfg(all(feature = "axum", feature = "admin"))]
mod api_admin_axum;
#[cfg(all(feature = "axum", feature = "admin"))]
pub use api_admin_axum::*;

#[derive(Clone)]
pub struct Nip98Auth {
    pub pubkey: [u8; 32],
    pub method_tag: String,
    pub url_tag: String,
    /// Optional NIP-98 `payload` tag: SHA-256 hash of the request body
    pub payload_tag: Option<[u8; 32]>,
}

impl Nip98Auth {
    /// Try to parse a base64 encoded nostr event
    pub fn try_from_token(token: &str) -> Result<Self> {
        let decoded = base64::engine::general_purpose::STANDARD.decode(token.as_bytes())?;
        if decoded.is_empty() || decoded[0] != b'{' {
            bail!("Invalid token");
        }

        let event: Event = Event::from_json(decoded)?;
        if event.verify().is_err() {
            bail!("Invalid nostr event, invalid signature");
        }
        if event.kind != Kind::Custom(27235) {
            bail!("Invalid nostr event, wrong kind");
        }
        let now = Utc::now();
        let event_time = DateTime::from_timestamp(event.created_at.as_secs() as i64, 0)
            .ok_or_else(|| anyhow!("Invalid timestamp"))?;
        let diff_seconds = (now - event_time).num_seconds().abs();
        if diff_seconds > 300 {
            bail!("Invalid nostr event, timestamp out of range");
        }

        // Check URL tag
        let url_tag = event
            .tags
            .iter()
            .find(|tag| {
                tag.kind() == TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::U))
            })
            .and_then(|tag| tag.content())
            .ok_or_else(|| anyhow!("Missing URL tag"))?;

        // Check method tag
        let method_tag = event
            .tags
            .iter()
            .find(|tag| tag.kind() == TagKind::Method)
            .and_then(|tag| tag.content())
            .ok_or_else(|| anyhow::anyhow!("Missing method tag"))?;

        // Optional payload tag (SHA-256 of the request body, hex encoded)
        let payload_tag = event
            .tags
            .iter()
            .find(|tag| tag.kind() == TagKind::Custom("payload".into()))
            .and_then(|tag| tag.content())
            .map(|h| {
                let bytes = hex::decode(h)?;
                let arr: [u8; 32] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("Invalid payload tag length"))?;
                Ok::<_, anyhow::Error>(arr)
            })
            .transpose()?;

        Ok(Self {
            pubkey: event.pubkey.to_bytes(),
            method_tag: method_tag.to_string(),
            url_tag: url_tag.to_string(),
            payload_tag,
        })
    }

    /// Verify the request body against the NIP-98 `payload` tag when present.
    ///
    /// The tag is optional per NIP-98 for backwards compatibility, but when a
    /// client includes it the body MUST match, which prevents replaying a
    /// captured token with a different request body.
    pub fn verify_payload(&self, body: &[u8]) -> Result<()> {
        use nostr_sdk::hashes::{Hash, sha256};
        if let Some(expected) = &self.payload_tag {
            let actual = sha256::Hash::hash(body);
            if actual.as_byte_array() != expected {
                bail!("Invalid nostr event, payload hash mismatch");
            }
        }
        Ok(())
    }
}

#[cfg(feature = "axum")]
use axum::http::*;

#[cfg(feature = "axum")]
impl<S> axum::extract::FromRequestParts<S> for Nip98Auth {
    type Rejection = (StatusCode, String);

    fn from_request_parts(
        parts: &mut request::Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        Box::pin(async {
            let auth = if let Some(a) = parts.headers.get("authorization") {
                a.to_str().map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid authorization header {}", e),
                    )
                })?
            } else {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "Missing authorization header".to_string(),
                ));
            };

            let Some((scheme, token)) = auth.split_once(" ") else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Invalid authorization header".to_string(),
                ));
            };
            if scheme != "Nostr" {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Invalid scheme {}", scheme),
                ));
            }

            match Nip98Auth::try_from_token(token) {
                Ok(auth) => {
                    if parts.method.as_str() != auth.method_tag {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            format!(
                                "Invalid auth method, {} != {}",
                                parts.method.as_str(),
                                auth.method_tag
                            ),
                        ));
                    }
                    let Ok(auth_url) = Uri::from_str(&auth.url_tag) else {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            format!("Invalid auth url, {}", auth.url_tag),
                        ));
                    };
                    if parts.uri.path() != auth_url.path() {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            format!(
                                "Invalid auth url, {} != {}",
                                parts.uri.path(),
                                auth_url.path()
                            ),
                        ));
                    }
                    // When the signed URL includes a query string it must match the
                    // request query, otherwise a token could be replayed with different
                    // query parameters. (Tokens without a query are accepted against
                    // any query for backwards compatibility with existing clients.)
                    if let Some(auth_query) = auth_url.query()
                        && Some(auth_query) != parts.uri.query()
                    {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            format!(
                                "Invalid auth url query, {} != {}",
                                parts.uri.query().unwrap_or(""),
                                auth_query
                            ),
                        ));
                    }
                    Ok(auth)
                }
                Err(e) => Err((
                    StatusCode::BAD_REQUEST,
                    format!("Could not parse authorization token {}", e),
                )),
            }
        })
    }
}

/// Extractor combining [Nip98Auth] with a JSON body.
///
/// Unlike using `Nip98Auth` + `Json<T>` separately, this buffers the raw body and
/// verifies it against the NIP-98 `payload` tag (when the client includes one),
/// preventing replay of a captured token with a different request body.
#[cfg(feature = "axum")]
pub struct Nip98Json<T> {
    pub auth: Nip98Auth,
    pub body: T,
}

#[cfg(feature = "axum")]
impl<S, T> axum::extract::FromRequest<S> for Nip98Json<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = (StatusCode, String);

    fn from_request(
        req: axum::extract::Request,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        Box::pin(async move {
            const MAX_BODY_SIZE: usize = 1024 * 1024;
            let (mut parts, body) = req.into_parts();
            let auth =
                <Nip98Auth as axum::extract::FromRequestParts<S>>::from_request_parts(
                    &mut parts, state,
                )
                .await?;
            let bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid body: {}", e)))?;
            auth.verify_payload(&bytes)
                .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;
            let body: T = serde_json::from_slice(&bytes)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e)))?;
            Ok(Self { auth, body })
        })
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct ApiError {
    error: String,
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error: error.to_string(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PageQueryV1 {
    pub page: i32,
    #[serde(alias = "pageSize")]
    pub limit: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::{EventBuilder, Keys, Tag};

    fn make_token(tags: Vec<Tag>) -> String {
        let keys = Keys::generate();
        let ev = EventBuilder::new(Kind::Custom(27235), "")
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(ev.as_json())
    }

    #[test]
    fn nip98_parses_payload_tag_and_verifies_body() {
        use nostr_sdk::hashes::{Hash, sha256};
        let body = br#"{"title":"test"}"#;
        let hash = sha256::Hash::hash(body);
        let token = make_token(vec![
            Tag::parse(["u", "https://example.com/api/v1/event"]).unwrap(),
            Tag::parse(["method", "PATCH"]).unwrap(),
            Tag::parse(["payload", &hash.to_string()]).unwrap(),
        ]);
        let auth = Nip98Auth::try_from_token(&token).unwrap();
        assert!(auth.payload_tag.is_some());
        // matching body passes
        auth.verify_payload(body).unwrap();
        // regression: a different body must be rejected (token replay with new body)
        assert!(auth.verify_payload(b"{\"title\":\"evil\"}").is_err());
    }

    #[test]
    fn nip98_without_payload_tag_allows_any_body() {
        let token = make_token(vec![
            Tag::parse(["u", "https://example.com/api/v1/event"]).unwrap(),
            Tag::parse(["method", "PATCH"]).unwrap(),
        ]);
        let auth = Nip98Auth::try_from_token(&token).unwrap();
        assert!(auth.payload_tag.is_none());
        auth.verify_payload(b"anything").unwrap();
    }

    #[test]
    fn nip98_rejects_bad_payload_tag() {
        let token = make_token(vec![
            Tag::parse(["u", "https://example.com/api/v1/event"]).unwrap(),
            Tag::parse(["method", "PATCH"]).unwrap(),
            Tag::parse(["payload", "zz-not-hex"]).unwrap(),
        ]);
        assert!(Nip98Auth::try_from_token(&token).is_err());
    }
}
