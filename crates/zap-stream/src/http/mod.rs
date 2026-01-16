use crate::stream_manager::StreamManager;
use anyhow::Result;
use axum::Router;
use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use serde::Serialize;

#[cfg(feature = "hls")]
mod hls;
#[cfg(feature = "hls")]
pub use hls::*;
mod zap;
pub use zap::*;
mod range;
pub use range::*;
mod multitrack;
pub use multitrack::*;

#[derive(Serialize, Clone)]
pub struct StreamData {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub live_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer_count: Option<u64>,
}

#[derive(Serialize, Clone)]
struct IndexTemplateData {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    streams: Vec<StreamData>,
}

/// Router which serves the stream list index page
#[derive(Clone)]
pub struct IndexRouter {
    stream_manager: StreamManager,
}

impl IndexRouter {
    pub fn new(stream_manager: StreamManager) -> Router {
        let me = IndexRouter { stream_manager };

        Router::new()
            .route("/", get(Self::index_route))
            .with_state(me)
    }

    async fn index_route(State(me): State<IndexRouter>) -> Result<Html<String>, String> {
        let template =
            mustache::compile_str(include_str!("index.html")).map_err(|e| e.to_string())?;

        let streams = me.stream_manager.get_active_streams().await;
        Ok(Html(
            template
                .render_to_string(&IndexTemplateData {
                    streams: streams
                        .into_iter()
                        .map(|s| StreamData {
                            id: s.stream_id,
                            title: s.title.unwrap_or(String::default()),
                            summary: None,
                            live_url: s.urls.into_iter().next().unwrap_or_default(),
                            viewer_count: Some(s.viewers as _),
                        })
                        .collect(),
                })
                .map_err(|e| e.to_string())?,
        ))
    }
}
