use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::get;
use axum_extra::response::FileStream;
use std::path::PathBuf;
use zap_stream_core::pipeline::PipelineRunner;

/// Simple fileserver serving image thumbnails
pub struct ThumbServer;

impl ThumbServer {
    pub fn new<P>(out_dir: P) -> Router
    where
        P: Send + Sync + Clone + 'static,
        PathBuf: From<P>,
    {
        Router::new().route(
            &format!("/{{stream_id}}/{}", PipelineRunner::THUMB_PATH),
            get(async move |Path(stream_id): Path<String>| {
                let path = PathBuf::from(out_dir)
                    .join(stream_id)
                    .join(PipelineRunner::THUMB_PATH);
                FileStream::from_path(path)
                    .await
                    .map_err(|_| StatusCode::NOT_FOUND)
            }),
        )
    }
}
