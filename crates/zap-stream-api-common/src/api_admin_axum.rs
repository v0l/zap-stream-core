use crate::{
    AdminIngestEndpointRequest, AdminUserRequest, Nip98Auth, ZapStreamAdminApi,
};
use crate::{ApiError, PageQueryV1};
use axum::extract::{Path, Query, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

/// Axum router which passes calls to the internal [ZapStreamAdminApi]
#[derive(Clone)]
pub struct AxumAdminApi<T>
where
    T: ZapStreamAdminApi + 'static,
{
    handler: T,
}

impl<T> AxumAdminApi<T>
where
    T: ZapStreamAdminApi + 'static,
{
    pub fn new(handler: T) -> Router {
        Router::new()
            .route(
                "/api/v1/admin/users",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Query(q): Query<GetUsersV1Query>| {
                        match this
                            .handler
                            .get_users(auth, q.page, q.limit, q.search)
                            .await
                        {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/users/{id}",
                patch(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>,
                           Json(req): Json<AdminUserRequest>| {
                        match this.handler.update_user(auth, id, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/users/{id}/history",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>,
                           Query(q): Query<PageQueryV1>| {
                        match this
                            .handler
                            .get_user_balance_history(auth, id, q.page as _, q.limit as _)
                            .await
                        {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/users/{id}/streams",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>,
                           Query(q): Query<PageQueryV1>| {
                        match this
                            .handler
                            .get_user_streams(auth, id, q.page as _, q.limit as _)
                            .await
                        {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/users/{id}/stream-key",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>| {
                        match this.handler.get_user_stream_key(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/users/{id}/stream-key/regenerate",
                post(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>| {
                        match this.handler.regenerate_user_stream_key(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/audit-log",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Query(q): Query<PageQueryV1>| {
                        match this
                            .handler
                            .get_audit_log(auth, q.page as _, q.limit as _)
                            .await
                        {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/ingest-endpoints",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Query(q): Query<PageQueryV1>| {
                        match this
                            .handler
                            .get_ingest_endpoints(auth, q.page as _, q.limit as _)
                            .await
                        {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                )
                .post(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Json(req): Json<AdminIngestEndpointRequest>| {
                        match this.handler.create_ingest_endpoint(auth, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/ingest-endpoints/{id}",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>| {
                        match this.handler.get_ingest_endpoint(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                )
                .patch(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>,
                           Json(req): Json<AdminIngestEndpointRequest>| {
                        match this.handler.update_ingest_endpoint(auth, id, req).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                )
                .delete(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<u64>| {
                        match this.handler.delete_ingest_endpoint(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/admin/pipeline-log/{stream_id}",
                get(
                    async |auth: Nip98Auth,
                           State(this): State<AxumAdminApi<T>>,
                           Path(id): Path<Uuid>| {
                        match this.handler.get_stream_logs(auth, id).await {
                            Ok(r) => Ok(Json(r)),
                            Err(e) => Err(Json(ApiError::from(e))),
                        }
                    },
                ),
            )
            .with_state(AxumAdminApi { handler })
    }
}

#[derive(Deserialize)]
struct GetUsersV1Query {
    page: u32,
    limit: u32,
    search: Option<String>,
}
