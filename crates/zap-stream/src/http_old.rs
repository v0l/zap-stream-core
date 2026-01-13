use crate::auth::{AuthRequest, AuthResult, TokenSource, authenticate_nip98};
use crate::settings::Settings;
use crate::viewer::ViewerTracker;
use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use http_range_header::{
    EndPosition, StartPosition, SyntacticallyCorrectRange, parse_range_header,
};
use hyper::body::{Frame, Incoming};
use hyper::http::response::Builder;
use hyper::service::Service;
use hyper::{Request, Response, StatusCode};
use matchit::Router;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::io::SeekFrom;
use std::ops::Range;
use std::path::PathBuf;
use std::pin::{Pin, pin};
use std::task::Poll;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tokio_util::io::ReaderStream;
use tracing::{error, warn};
use uuid::Uuid;

/// Plugin providing stream information to the http server
pub trait HttpServerPlugin: Clone {
    fn get_active_streams(&self) -> Pin<Box<dyn Future<Output = Result<Vec<StreamData>>> + Send>>;
    fn track_viewer(
        &self,
        stream_id: &str,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>;
    fn handler(self, request: Request<Incoming>) -> HttpFuture;
    fn handle_websocket_metrics(self, request: Request<Incoming>) -> HttpFuture;
}

#[derive(Clone)]
pub enum HttpServerPath {
    Index,
    HlsMasterPlaylist,
    HlsVariantPlaylist,
    HlsSegmentFile,
    WebSocketMetrics,
}

#[derive(Clone)]
pub struct HttpServer<T> {
    files_dir: PathBuf,
    plugin: T,
    router: Router<HttpServerPath>,
}

impl<T> HttpServer<T>
where
    T: HttpServerPlugin,
{
    pub fn new(files_dir: PathBuf, plugin: T) -> Self {
        let mut router = Router::new();
        router
            .insert("/", HttpServerPath::Index)
            .expect("invalid route");
        router
            .insert("/index.html", HttpServerPath::Index)
            .expect("invalid route");
        #[cfg(feature = "hls")]
        {
            use zap_stream_core::egress::hls::HlsEgress;
            router
                .insert(
                    format!("/{{stream}}/{}/live.m3u8", HlsEgress::PATH),
                    HttpServerPath::HlsMasterPlaylist,
                )
                .expect("invalid route");
            router
                .insert(
                    format!("/{{stream}}/{}/{{variant}}/live.m3u8", HlsEgress::PATH),
                    HttpServerPath::HlsVariantPlaylist,
                )
                .expect("invalid route");
            router
                .insert(
                    format!("/{{stream}}/{}/{{variant}}/{{seg}}", HlsEgress::PATH),
                    HttpServerPath::HlsSegmentFile,
                )
                .expect("invalid route");
        }
        router
            .insert("/api/v1/ws", HttpServerPath::WebSocketMetrics)
            .expect("invalid route");

        Self {
            files_dir,
            plugin,
            router,
        }
    }

    async fn handle_index(
        plugin: &T,
        template: String,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        // Compile template outside async move for better performance

    }

    async fn handle_hls_segment(
        req: &Request<Incoming>,
        segment_path: PathBuf,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {

    }

    async fn handle_hls_master_playlist(
        req: &Request<Incoming>,
        playlist_path: PathBuf,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        // Get client IP and User-Agent for tracking

    }


    pub fn base_response() -> Builder {
        Response::builder()
            .header("server", "zap-stream-core")
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-headers", "*")
            .header("access-control-allow-methods", "HEAD, GET, OPTIONS")
    }

    /// Get a response object for a file body
    async fn path_to_response(path: PathBuf) -> Result<Response<BoxBody<Bytes, anyhow::Error>>> {
        if !path.exists() {
            return Ok(Self::base_response().status(404).body(BoxBody::default())?);
        }
        let f = File::open(&path).await?;
        let f_stream = ReaderStream::new(f);
        let body = StreamBody::new(
            f_stream
                .map_ok(Frame::data)
                .map_err(|e| anyhow::anyhow!("Failed to read body: {}", e)),
        )
        .boxed();
        Ok(Self::base_response().body(body)?)
    }
}
