use crate::multitrack::{MultiTrackConfigRequest, MultiTrackEngine};
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};

#[derive(Clone)]
pub struct MultiTrackRouter {
    engine: MultiTrackEngine,
}

impl MultiTrackRouter {
    pub fn new(engine: MultiTrackEngine) -> Router {
        Router::new()
            .route(
                "/api/v1/multi-track-config",
                post(
                    async |State(this): State<MultiTrackRouter>,
                           Json(req): Json<MultiTrackConfigRequest>| {
                        Ok::<Json<_>, &'static str>(Json(
                            this.engine
                                .get_multi_track_config(req)
                                .await
                                .map_err(|_| "Invalid request")?,
                        ))
                    },
                ),
            )
            .with_state(MultiTrackRouter { engine })
    }
}
