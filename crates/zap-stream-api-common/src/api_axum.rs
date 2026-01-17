use crate::{ApiError, CreateStreamKeyRequest, PageQueryV1, PatchAccount};
use crate::{ForwardRequest, Nip98Auth, PatchEvent, UpdateForwardRequest, ZapStreamApi};
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Axum router which passes calls to the internal [ZapStreamApi]
#[derive(Clone)]
pub struct AxumApi<T>
where
    T: ZapStreamApi + 'static,
{
    handler: T,
}

impl<T> AxumApi<T>
where
    T: ZapStreamApi + 'static,
{
    pub fn new(handler: T) -> Router {
        Router::new()
            .route(
                "/api/v1/account",
                get(async |auth: Nip98Auth, State(this): State<AxumApi<T>>| {
                    match this.handler.get_account(auth).await {
                        Ok(r) => Ok(Json(r)),
                        Err(e) => Err(Json(ApiError::from(e))),
                    }
                })
                .patch(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Json(req): Json<PatchAccount>| {
                        match this.handler.update_account(auth, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/event",
                patch(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Json(req): Json<PatchEvent>| {
                        match this.handler.update_event(auth, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/account/forward",
                post(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Json(req): Json<ForwardRequest>| {
                        match this.handler.create_forward(auth, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/account/forward/{id}",
                delete(
                    async |auth: Nip98Auth, State(this): State<AxumApi<T>>, Path(id): Path<u64>| {
                        match this.handler.delete_forward(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                )
                .patch(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Path(id): Path<u64>,
                           Json(req): Json<UpdateForwardRequest>| {
                        match this.handler.update_forward(auth, id, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/history",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Query(q): Query<PageQueryV1>| {
                        match this
                            .handler
                            .get_balance_history(auth, q.page as _, q.limit as _)
                            .await
                        {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/keys",
                get(async |auth: Nip98Auth, State(this): State<AxumApi<T>>| {
                    match this.handler.get_stream_keys(auth).await {
                        Ok(r) => Ok(Json(r)),
                        Err(e) => Err(Json(ApiError::from(e))),
                    }
                })
                .post(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Json(req): Json<CreateStreamKeyRequest>| {
                        match this.handler.create_stream_key(auth, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/time",
                get(async || {
                    Json(TimeResponse {
                        time: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as _,
                    })
                }),
            )
            .route(
                "/api/v1/stream/{id}",
                delete(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Path(id): Path<Uuid>| {
                        match this.handler.delete_event(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/topup",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumApi<T>>,
                           Query(q): Query<TopupV1Query>| {
                        match this.handler.topup(auth.pubkey, q.amount * 1000, None).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/games/search",
                get(
                    async |State(this): State<AxumApi<T>>, Query(q): Query<SearchGamesV1Query>| {
                        match this.handler.search_games(q.q).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/games/{id}",
                get(
                    async |State(this): State<AxumApi<T>>, Path(id): Path<String>| match this
                        .handler
                        .get_game(id)
                        .await
                    {
                        Ok(r) => Ok(Json(r)),
                        Err(e) => Err(Json(ApiError::from(e))),
                    },
                ),
            )
            .with_state(AxumApi { handler })
    }
}

#[derive(Deserialize)]
struct TopupV1Query {
    amount: u64,
}

#[derive(Deserialize)]
struct SearchGamesV1Query {
    q: String,
}

#[derive(Serialize)]
struct TimeResponse {
    time: u64,
}
