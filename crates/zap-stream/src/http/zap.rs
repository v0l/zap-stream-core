use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use lnurl::pay::{LnURLPayInvoice, PayResponse};
use nostr_sdk::{Client, PublicKey, serde_json};
use serde::{Deserialize, Serialize};
use zap_stream_api_common::ZapStreamApi;
use zap_stream_db::ZapStreamDb;

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
                            max_sendable: 1_000_000_000,
                            min_sendable: 1_000,
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
