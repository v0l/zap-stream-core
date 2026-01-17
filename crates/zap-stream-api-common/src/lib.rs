use anyhow::{Result, anyhow, bail};
use base64::Engine;
use chrono::{DateTime, Utc};
use nostr_sdk::{Alphabet, Event, JsonUtil, Kind, SingleLetterTag, TagKind};
use serde::{Deserialize, Serialize};
use std::fmt::format;
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

        Ok(Self {
            pubkey: event.pubkey.to_bytes(),
            method_tag: method_tag.to_string(),
            url_tag: url_tag.to_string(),
        })
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
