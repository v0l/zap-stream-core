use crate::auth::{authenticate_nip98, AuthRequest, AuthResult, TokenSource};
use crate::viewer::ViewerTracker;
use anyhow::{bail, ensure, Context, Result};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use http_range_header::{
    parse_range_header, EndPosition, StartPosition, SyntacticallyCorrectRange,
};
use hyper::body::{Frame, Incoming};
use hyper::http::response::Builder;
use hyper::service::Service;
use hyper::{Request, Response, StatusCode};
use log::{error, warn};
use matchit::Router;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::io::SeekFrom;
use std::ops::Range;
use std::path::PathBuf;
use std::pin::{pin, Pin};
use std::task::Poll;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;

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
        router.insert("/", HttpServerPath::Index).unwrap();
        router.insert("/index.html", HttpServerPath::Index).unwrap();
        router
            .insert(
                format!("/{{stream}}/{}/live.m3u8", HlsEgress::PATH),
                HttpServerPath::HlsMasterPlaylist,
            )
            .unwrap();
        router
            .insert(
                format!("/{{stream}}/{}/{{variant}}/live.m3u8", HlsEgress::PATH),
                HttpServerPath::HlsVariantPlaylist,
            )
            .unwrap();
        router
            .insert(
                format!("/{{stream}}/{}/{{variant}}/{{seg}}.ts", HlsEgress::PATH),
                HttpServerPath::HlsSegmentFile,
            )
            .unwrap();
        router
            .insert(
                format!("/{{stream}}/{}/{{variant}}/{{seg}}.m4s", HlsEgress::PATH),
                HttpServerPath::HlsSegmentFile,
            )
            .unwrap();
        router
            .insert("/api/v1/ws", HttpServerPath::WebSocketMetrics)
            .unwrap();

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
        let template = match mustache::compile_str(&template) {
            Ok(t) => t,
            Err(e) => {
                error!("Failed to compile template: {}", e);
                return Ok(Self::base_response().status(500).body(BoxBody::default())?);
            }
        };

        let streams = plugin.get_active_streams().await;
        match streams {
            Ok(data) => match template.render_to_string(&IndexTemplateData { streams: data }) {
                Ok(index_html) => Ok(Self::base_response()
                    .header("content-type", "text/html")
                    .body(
                        Full::new(Bytes::from(index_html))
                            .map_err(|e| match e {})
                            .boxed(),
                    )?),
                Err(e) => {
                    error!("Failed to render template: {}", e);
                    Ok(Self::base_response().status(500).body(BoxBody::default())?)
                }
            },
            Err(e) => {
                error!("Failed to fetch template data: {}", e);
                Ok(Self::base_response().status(500).body(BoxBody::default())?)
            }
        }
    }

    async fn handle_hls_segment(
        req: &Request<Incoming>,
        segment_path: PathBuf,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        let mut response = Self::base_response().header("accept-ranges", "bytes");

        if let Some(r) = req.headers().get("range") {
            if let Ok(ranges) = parse_range_header(r.to_str()?) {
                if ranges.ranges.len() > 1 {
                    warn!("Multipart ranges are not supported, fallback to non-range request");
                    Self::path_to_response(segment_path).await
                } else {
                    let file = File::open(&segment_path).await?;
                    let metadata = file.metadata().await?;
                    let single_range = ranges.ranges.first().unwrap();
                    let range = match RangeBody::get_range(metadata.len(), single_range) {
                        Ok(r) => r,
                        Err(e) => {
                            warn!("Failed to get range: {}", e);
                            return Ok(response
                                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                                .body(BoxBody::default())?);
                        }
                    };
                    let r_body = RangeBody::new(file, metadata.len(), range.clone());

                    response = response.status(StatusCode::PARTIAL_CONTENT);
                    let headers = r_body.get_headers();
                    for (k, v) in headers {
                        response = response.header(k, v);
                    }
                    let f_stream = ReaderStream::new(r_body);
                    let body = StreamBody::new(
                        f_stream
                            .map_ok(Frame::data)
                            .map_err(|e| anyhow::anyhow!("Failed to read body: {}", e)),
                    )
                    .boxed();
                    Ok(response.body(body)?)
                }
            } else {
                Ok(Self::base_response().status(400).body(BoxBody::default())?)
            }
        } else {
            Self::path_to_response(segment_path).await
        }
    }

    async fn handle_hls_master_playlist(
        req: &Request<Incoming>,
        playlist_path: PathBuf,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        // Get client IP and User-Agent for tracking
        let client_ip = Self::get_client_ip(req);
        let user_agent = req
            .headers()
            .get("user-agent")
            .and_then(|h| h.to_str().ok());

        let token = ViewerTracker::generate_viewer_token(&client_ip, user_agent);

        // Read the playlist file
        let playlist_content = tokio::fs::read(playlist_path).await?;

        // Parse and modify playlist to add viewer token to URLs
        let modified_content = Self::add_viewer_token_to_playlist(&playlist_content, &token)?;

        let response = Self::base_response()
            .header("content-type", "application/vnd.apple.mpegurl")
            .body(
                Full::new(match modified_content {
                    Cow::Borrowed(b) => Bytes::copy_from_slice(b.as_bytes()),
                    Cow::Owned(o) => Bytes::from(o),
                })
                .map_err(|e| match e {})
                .boxed(),
            )?;

        Ok(response)
    }

    fn get_client_ip(req: &Request<Incoming>) -> String {
        // Check common headers for real client IP
        if let Some(forwarded) = req.headers().get("x-forwarded-for") {
            if let Ok(forwarded_str) = forwarded.to_str() {
                if let Some(first_ip) = forwarded_str.split(',').next() {
                    return first_ip.trim().to_string();
                }
            }
        }

        if let Some(real_ip) = req.headers().get("x-real-ip") {
            if let Ok(ip_str) = real_ip.to_str() {
                return ip_str.to_string();
            }
        }

        // use random string as IP to avoid broken view tracker due to proxying
        Uuid::new_v4().to_string()
    }

    fn add_viewer_token_to_playlist<'a>(
        content: &'a [u8],
        viewer_token: &str,
    ) -> Result<Cow<'a, str>> {
        // Parse the M3U8 playlist using the m3u8-rs crate
        let (_, playlist) = m3u8_rs::parse_playlist(content)
            .map_err(|e| anyhow::anyhow!("Failed to parse M3U8 playlist: {}", e))?;

        match playlist {
            m3u8_rs::Playlist::MasterPlaylist(mut master) => {
                // For master playlists, add viewer token to variant streams
                for variant in &mut master.variants {
                    variant.uri = Self::add_token_to_url(&variant.uri, viewer_token);
                }

                // Write the modified playlist back to string
                let mut output = Vec::new();
                master
                    .write_to(&mut output)
                    .map_err(|e| anyhow::anyhow!("Failed to write master playlist: {}", e))?;
                String::from_utf8(output)
                    .map(Cow::Owned)
                    .map_err(|e| anyhow::anyhow!("Failed to convert playlist to string: {}", e))
            }
            m3u8_rs::Playlist::MediaPlaylist(_) => Ok(Cow::Borrowed(str::from_utf8(content)?)),
        }
    }

    fn add_token_to_url(url: &str, viewer_token: &str) -> String {
        if url.contains('?') {
            format!("{}&vt={}", url, viewer_token)
        } else {
            format!("{}?vt={}", url, viewer_token)
        }
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
type HttpResponse = Response<BoxBody<Bytes, HttpError>>;
type HttpError = anyhow::Error;
pub(crate) type HttpFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send>>;

impl<T> Service<Request<Incoming>> for HttpServer<T>
where
    T: HttpServerPlugin + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = HttpError;
    type Future = HttpFuture;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let path = req.uri().path().to_owned();
        let dst_path = self.files_dir.join(&req.uri().path()[1..]);

        if let Ok(m) = self.router.at(&path) {
            match m.value {
                HttpServerPath::Index => {
                    let plugin = self.plugin.clone();
                    let template = include_str!("../index.html");
                    let template = template.to_string();
                    return Box::pin(async move { Self::handle_index(&plugin, template).await });
                }
                HttpServerPath::HlsMasterPlaylist => {
                    let stream_id = m.params.get("stream").map(|s| s.to_string());
                    let file_path = dst_path.clone();
                    return Box::pin(async move {
                        let _stream_id = stream_id.context("stream id missing")?;
                        Self::handle_hls_master_playlist(&req, file_path).await
                    });
                }
                HttpServerPath::HlsVariantPlaylist => {
                    // extract the viewer token and track every hit
                    let stream_id = m.params.get("stream").map(|s| s.to_string());
                    let query_params: HashMap<String, String> = req
                        .uri()
                        .query()
                        .map(|q| {
                            url::form_urlencoded::parse(q.as_bytes())
                                .into_owned()
                                .collect()
                        })
                        .unwrap_or_default();
                    let plugin = self.plugin.clone();
                    return Box::pin(async move {
                        if let (Some(stream_id), Some(vt)) = (stream_id, query_params.get("vt")) {
                            plugin.track_viewer(&stream_id, vt).await?;
                        }
                        Self::path_to_response(dst_path).await
                    });
                }
                HttpServerPath::HlsSegmentFile => {
                    // handle segment file (range requests)
                    let file_path = dst_path.clone();
                    return Box::pin(
                        async move { Self::handle_hls_segment(&req, file_path).await },
                    );
                }
                HttpServerPath::WebSocketMetrics => {
                    let plugin = self.plugin.clone();
                    return plugin.handle_websocket_metrics(req);
                }
            }
        }

        // check if mapped to file (not handled route)
        if dst_path.exists() {
            return Box::pin(async move { Self::path_to_response(dst_path).await });
        }

        // fallback to api handler
        let plugin = self.plugin.clone();
        Box::pin(async move {
            match plugin.handler(req).await {
                Ok(res) => Ok(res),
                Err(e) => {
                    error!("{}", e);
                    Ok(Response::builder().status(500).body(BoxBody::default())?)
                }
            }
        })
    }
}

pub async fn check_nip98_auth(
    req: &Request<Incoming>,
    public_url: &str,
    db: &zap_stream_db::ZapStreamDb,
) -> Result<AuthResult> {
    let auth = if let Some(a) = req.headers().get("authorization") {
        a.to_str()?
    } else {
        bail!("Authorization header missing");
    };

    // Construct full URI using public_url + path + query
    let request_uri = match req.uri().query() {
        Some(query) => format!(
            "{}{}?{}",
            public_url.trim_end_matches('/'),
            req.uri().path(),
            query
        ),
        None => format!("{}{}", public_url.trim_end_matches('/'), req.uri().path()),
    };

    let auth_request = AuthRequest {
        token_source: TokenSource::HttpHeader(auth.to_string()),
        expected_url: request_uri,
        expected_method: req.method().as_str().to_string(),
    };

    authenticate_nip98(auth_request, db).await
}

/// Range request handler over file handle
struct RangeBody {
    file: File,
    range_start: u64,
    range_end: u64,
    current_offset: u64,
    poll_complete: bool,
    file_size: u64,
}

const MAX_UNBOUNDED_RANGE: u64 = 1024 * 1024;
impl RangeBody {
    pub fn new(file: File, file_size: u64, range: Range<u64>) -> Self {
        Self {
            file,
            file_size,
            range_start: range.start,
            range_end: range.end,
            current_offset: 0,
            poll_complete: false,
        }
    }

    pub fn get_range(file_size: u64, header: &SyntacticallyCorrectRange) -> Result<Range<u64>> {
        let range_start = match header.start {
            StartPosition::Index(i) => {
                ensure!(i < file_size, "Range start out of range");
                i
            }
            StartPosition::FromLast(i) => file_size.saturating_sub(i),
        };
        let range_end = match header.end {
            EndPosition::Index(i) => {
                ensure!(i <= file_size, "Range end out of range");
                i
            }
            EndPosition::LastByte => {
                (file_size.saturating_sub(1)).min(range_start + MAX_UNBOUNDED_RANGE)
            }
        };
        Ok(range_start..range_end)
    }

    pub fn get_headers(&self) -> Vec<(&'static str, String)> {
        let r_len = (self.range_end - self.range_start) + 1;
        vec![
            ("content-length", r_len.to_string()),
            (
                "content-range",
                format!(
                    "bytes {}-{}/{}",
                    self.range_start, self.range_end, self.file_size
                ),
            ),
        ]
    }
}

impl AsyncRead for RangeBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let range_start = self.range_start + self.current_offset;
        let range_len = self.range_end.saturating_sub(range_start) + 1;
        let bytes_to_read = buf.remaining().min(range_len as usize) as u64;

        if bytes_to_read == 0 {
            return Poll::Ready(Ok(()));
        }

        // when no pending poll, seek to starting position
        if !self.poll_complete {
            let pinned = pin!(&mut self.file);
            pinned.start_seek(SeekFrom::Start(range_start))?;
            self.poll_complete = true;
        }

        // check poll completion
        if self.poll_complete {
            let pinned = pin!(&mut self.file);
            match pinned.poll_complete(cx) {
                Poll::Ready(Ok(_)) => {
                    self.poll_complete = false;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }

        // Read data from the file
        let pinned = pin!(&mut self.file);
        match pinned.poll_read(cx, buf) {
            Poll::Ready(Ok(_)) => {
                self.current_offset += bytes_to_read;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                self.poll_complete = true;
                Poll::Pending
            }
        }
    }
}
