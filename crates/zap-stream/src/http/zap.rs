use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use lnurl::pay::{LnURLPayInvoice, PayResponse};
use nostr_sdk::{Client, Event, JsonUtil, Kind, PublicKey, serde_json};
use serde::{Deserialize, Serialize};
use zap_stream_api_common::ZapStreamApi;
use zap_stream_db::ZapStreamDb;

/// LUD-06 advertised bounds (millisats)
const MIN_SENDABLE: u64 = 1_000;
const MAX_SENDABLE: u64 = 1_000_000_000;

/// Validate the optional NIP-57 zap request passed on the LNURL callback.
/// Returns the (validated) original JSON or an error message.
fn validate_zap_request(json: &str) -> Result<(), &'static str> {
    let ev = Event::from_json(json).map_err(|_| "invalid zap request json")?;
    if ev.kind != Kind::ZapRequest {
        return Err("invalid zap request kind");
    }
    if ev.verify().is_err() {
        return Err("invalid zap request signature");
    }
    Ok(())
}

#[derive(Clone)]
pub struct ZapRouter<T>
where
    T: ZapStreamApi + 'static,
{
    public_url: String,
    client: Client,
    db: ZapStreamDb,
    handler: T,
}

impl<T> ZapRouter<T>
where
    T: ZapStreamApi + 'static,
{
    pub fn new(public_url: String, client: Client, db: ZapStreamDb, handler: T) -> Router {
        Router::new()
            .route(
                "/.well-known/lnurlp/{name}",
                get(
                    async |State(this): State<ZapRouter<T>>, Path(name): Path<String>| {
                        if let Ok(pk) = hex::decode(&name)
                            && this
                                .db
                                .get_user_by_pubkey(
                                    &pk.as_slice()
                                        .try_into()
                                        .map_err(|_| Json(Lud06Error::error("invalid pubkey")))?,
                                )
                                .await
                                .map_err(|_| Json(Lud06Error::error("database error")))?
                                .is_some()
                        {
                        } else {
                            return Err(Json(Lud06Error::error("user not found")));
                        };

                        let meta =
                            vec![vec!["text/plain".to_string(), format!("Zap for {}", name)]];
                        let pubkey = this
                            .client
                            .signer()
                            .await
                            .map_err(|_| Json(Lud06Error::error("nostr client error")))?
                            .get_public_key()
                            .await
                            .map_err(|_| Json(Lud06Error::error("nostr client error")))?;
                        let full_url = format!(
                            "{}/api/v1/zap/{}",
                            this.public_url.trim_end_matches('/'),
                            name
                        );
                        let rsp = PayResponse {
                            callback: full_url,
                            max_sendable: MAX_SENDABLE,
                            min_sendable: MIN_SENDABLE,
                            tag: lnurl::Tag::PayRequest,
                            metadata: serde_json::to_string(&meta)
                                .map_err(|_| Json(Lud06Error::error("metadata error")))?,
                            comment_allowed: None,
                            allows_nostr: Some(true),
                            nostr_pubkey: Some(
                                pubkey
                                    .xonly()
                                    .map_err(|_| Json(Lud06Error::error("pubkey error")))?,
                            ),
                        };
                        Ok(Json(rsp))
                    },
                ),
            )
            .route(
                "/api/v1/zap/{pubkey}",
                get(
                    async |State(this): State<ZapRouter<T>>,
                           Path(name): Path<String>,
                           Query(q): Query<ZapQuery>| {
                        let target_user = if let Ok(pk) = hex::decode(&name) {
                            this.db
                                .get_user_by_pubkey(
                                    &pk.as_slice()
                                        .try_into()
                                        .map_err(|_| Json(Lud06Error::error("invalid pubkey")))?,
                                )
                                .await
                                .map_err(|_| Json(Lud06Error::error("user not found")))?
                        } else {
                            None
                        };
                        let target_user = if let Some(tu) = target_user {
                            tu
                        } else {
                            return Err(Json(Lud06Error::error("user not found")));
                        };
                        let user_pubkey = PublicKey::from_slice(&target_user.pubkey)
                            .map_err(|_| Json(Lud06Error::error("invalid user pubkey")))?;
                        // enforce the advertised LUD-06 min/max bounds
                        if q.amount < MIN_SENDABLE || q.amount > MAX_SENDABLE {
                            return Err(Json(Lud06Error::error("amount out of range")));
                        }
                        // reject malformed zap requests up-front instead of silently
                        // failing at receipt time
                        if let Some(zap_json) = q.nostr.as_deref()
                            && let Err(e) = validate_zap_request(zap_json)
                        {
                            return Err(Json(Lud06Error::error(e)));
                        }
                        let topup = this
                            .handler
                            .topup(user_pubkey.to_bytes(), q.amount, q.nostr)
                            .await
                            .map_err(|_| Json(Lud06Error::error("internal handler error")))?;
                        let rsp = LnURLPayInvoice::new(topup.pr);
                        Ok(Json(rsp))
                    },
                ),
            )
            .with_state(ZapRouter {
                public_url,
                client,
                db,
                handler,
            })
    }
}

#[derive(Deserialize)]
struct ZapQuery {
    amount: u64,
    nostr: Option<String>,
}

#[derive(Serialize)]
struct Lud06Error {
    status: String,
    reason: String,
}

impl Lud06Error {
    fn error(msg: &str) -> Self {
        Self {
            status: "ERROR".to_string(),
            reason: msg.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::{EventBuilder, Keys};

    #[test]
    fn validate_zap_request_rejects_garbage() {
        assert!(validate_zap_request("not json").is_err());
        assert!(validate_zap_request("{}").is_err());
    }

    #[test]
    fn validate_zap_request_rejects_wrong_kind() {
        let keys = Keys::generate();
        let ev = EventBuilder::text_note("hello")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(validate_zap_request(&ev.as_json()).is_err());
    }

    #[test]
    fn validate_zap_request_accepts_valid() {
        let keys = Keys::generate();
        let ev = EventBuilder::new(Kind::ZapRequest, "")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(validate_zap_request(&ev.as_json()).is_ok());
    }
}
