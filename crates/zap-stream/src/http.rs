use crate::api::Api;
use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use bytes::Bytes;
use chrono::{DateTime, Utc};
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
use nostr_sdk::{serde_json, Alphabet, Event, Kind, PublicKey, SingleLetterTag, TagKind};
use serde::Serialize;
use std::future::Future;
use std::io::SeekFrom;
use std::ops::Range;
use std::path::PathBuf;
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::viewer::ViewerTracker;

#[derive(Serialize, Clone)]
struct StreamData {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    live_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_count: Option<u64>,
}

#[derive(Serialize, Clone)]
struct IndexTemplateData {
    public_url: String,
    has_streams: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    streams: Vec<StreamData>,
}

pub struct CachedStreams {
    data: IndexTemplateData,
    cached_at: Instant,
}

#[derive(Clone)]
pub enum HttpServerPath {
    Index,
    HlsMasterPlaylist,
    HlsVariantPlaylist,
    HlsSegmentFile,
}

pub type StreamCache = Arc<RwLock<Option<CachedStreams>>>;

#[derive(Clone)]
pub struct HttpServer {
    index_template: String,
    files_dir: PathBuf,
    api: Api,
    stream_cache: StreamCache,
    router: Router<HttpServerPath>,
}

impl HttpServer {
    pub fn new(
        index_template: String,
        files_dir: PathBuf,
        api: Api,
        stream_cache: StreamCache,
    ) -> Self {
        let mut router = Router::new();
        router.insert("/", HttpServerPath::Index).unwrap();
        router.insert("/index.html", HttpServerPath::Index).unwrap();
        router
            .insert(
                format!("/{}/{{stream}}/live.m3u8", HlsEgress::PATH),
                HttpServerPath::HlsMasterPlaylist,
            )
            .unwrap();
        router
            .insert(
                format!("/{}/{{stream}}/{{variant}}/live.m3u8", HlsEgress::PATH),
                HttpServerPath::HlsVariantPlaylist,
            )
            .unwrap();
        router
            .insert(
                format!("/{}/{{stream}}/{{variant}}/{{seg}}.ts", HlsEgress::PATH),
                HttpServerPath::HlsSegmentFile,
            )
            .unwrap();

        Self {
            index_template,
            files_dir,
            api,
            stream_cache,
            router,
        }
    }

    async fn get_cached_or_fetch_streams_static(
        stream_cache: &StreamCache,
        api: &Api,
    ) -> Result<IndexTemplateData> {
        const CACHE_DURATION: Duration = Duration::from_secs(10);

        // Check if we have valid cached data
        {
            let cache = stream_cache.read().await;
            if let Some(ref cached) = *cache {
                if cached.cached_at.elapsed() < CACHE_DURATION {
                    return Ok(cached.data.clone());
                }
            }
        }

        // Cache is expired or missing, fetch new data
        let active_streams = api.get_active_streams().await?;
        let public_url = api.get_public_url();

        let template_data = if !active_streams.is_empty() {
            let streams: Vec<StreamData> = active_streams
                .into_iter()
                .map(|stream| {
                    let viewer_count = api.get_viewer_count(&stream.id);
                    // TODO: remove HLS assumption
                    StreamData {
                        id: stream.id.clone(),
                        title: stream
                            .title
                            .unwrap_or_else(|| format!("Stream {}", &stream.id[..8])),
                        summary: stream.summary,
                        live_url: format!("/{}/{}/live.m3u8", HlsEgress::PATH, stream.id),
                        viewer_count: if viewer_count > 0 {
                            Some(viewer_count as _)
                        } else {
                            None
                        },
                    }
                })
                .collect();

            IndexTemplateData {
                public_url,
                has_streams: true,
                streams,
            }
        } else {
            IndexTemplateData {
                public_url,
                has_streams: false,
                streams: Vec::new(),
            }
        };

        // Update cache
        {
            let mut cache = stream_cache.write().await;
            *cache = Some(CachedStreams {
                data: template_data.clone(),
                cached_at: Instant::now(),
            });
        }

        Ok(template_data)
    }

    async fn handle_index(
        api: Api,
        stream_cache: StreamCache,
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

        let template_data = Self::get_cached_or_fetch_streams_static(&stream_cache, &api).await;

        match template_data {
            Ok(data) => match template.render_to_string(&data) {
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
        api: Api,
        req: &Request<Incoming>,
        stream_id: &str,
        playlist_path: PathBuf,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        // Get client IP and User-Agent for tracking
        let client_ip = Self::get_client_ip(req);
        let user_agent = req
            .headers()
            .get("user-agent")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        // Check for existing viewer token in query params
        let query_params: std::collections::HashMap<String, String> = req
            .uri()
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();

        let viewer_token = if let Some(token) = query_params.get("vt") {
            // Track existing viewer
            api.track_viewer(token, stream_id, &client_ip, user_agent.clone());
            token.clone()
        } else {
            // Generate new viewer token based on IP and user agent fingerprint
            let token = ViewerTracker::generate_viewer_token(&client_ip, user_agent.as_deref());
            api.track_viewer(&token, stream_id, &client_ip, user_agent);
            token
        };

        // Read the playlist file
        let playlist_content = tokio::fs::read(playlist_path).await?;

        // Parse and modify playlist to add viewer token to URLs
        let modified_content =
            Self::add_viewer_token_to_playlist(&playlist_content, &viewer_token)?;

        let response = Self::base_response()
            .header("content-type", "application/vnd.apple.mpegurl")
            .body(
                Full::new(Bytes::from(modified_content))
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

    fn add_viewer_token_to_playlist(content: &[u8], viewer_token: &str) -> Result<String> {
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
                    .map_err(|e| anyhow::anyhow!("Failed to convert playlist to string: {}", e))
            }
            m3u8_rs::Playlist::MediaPlaylist(_) => {
                // For media playlists, return original content unchanged
                String::from_utf8(content.to_vec())
                    .map_err(|e| anyhow::anyhow!("Failed to convert playlist to string: {}", e))
            }
        }
    }

    fn add_token_to_url(url: &str, viewer_token: &str) -> String {
        if url.contains('?') {
            format!("{}&vt={}", url, viewer_token)
        } else {
            format!("{}?vt={}", url, viewer_token)
        }
    }

    fn base_response() -> Builder {
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

impl Service<Request<Incoming>> for HttpServer {
    type Response = Response<BoxBody<Bytes, Self::Error>>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let path = req.uri().path().to_owned();
        // request path as a file path pointing to the output directory
        let dst_path = self.files_dir.join(req.uri().path()[1..].to_string());

        if let Ok(m) = self.router.at(&path) {
            match m.value {
                HttpServerPath::Index => {
                    let api = self.api.clone();
                    let cache = self.stream_cache.clone();
                    let template = self.index_template.clone();
                    return Box::pin(async move { Self::handle_index(api, cache, template).await });
                }
                HttpServerPath::HlsMasterPlaylist => {
                    let api = self.api.clone();
                    let stream_id = m.params.get("stream").map(|s| s.to_string());
                    let file_path = dst_path.clone();
                    return Box::pin(async move {
                        let stream_id = stream_id.context("stream id missing")?;
                        Ok(
                            Self::handle_hls_master_playlist(api, &req, &stream_id, file_path)
                                .await?,
                        )
                    });
                }
                HttpServerPath::HlsVariantPlaylist => {
                    // let file handler handle this one, may be used later for HLS-LL to create
                    // delta updates
                }
                HttpServerPath::HlsSegmentFile => {
                    // handle segment file (range requests)
                    let file_path = dst_path.clone();
                    return Box::pin(async move {
                        Ok(Self::handle_hls_segment(&req, file_path).await?)
                    });
                }
            }
        }

        // check if mapped to file (not handled route)
        if dst_path.exists() {
            return Box::pin(async move { Self::path_to_response(dst_path).await });
        }

        // fallback to api handler
        let api = self.api.clone();
        Box::pin(async move {
            match api.handler(req).await {
                Ok(res) => Ok(res),
                Err(e) => {
                    error!("{}", e);
                    Ok(Response::builder().status(500).body(BoxBody::default())?)
                }
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct AuthResult {
    pub pubkey: PublicKey,
    pub event: Event,
}

pub fn check_nip98_auth(req: &Request<Incoming>, public_url: &str) -> Result<AuthResult> {
    let auth = if let Some(a) = req.headers().get("authorization") {
        a.to_str()?
    } else {
        bail!("Authorization header missing");
    };

    if !auth.starts_with("Nostr ") {
        bail!("Invalid authorization scheme");
    }

    let token = &auth[6..];
    let decoded = base64::engine::general_purpose::STANDARD.decode(token.as_bytes())?;

    // Check if decoded data starts with '{'
    if decoded.is_empty() || decoded[0] != b'{' {
        bail!("Invalid token");
    }

    let json = String::from_utf8(decoded)?;
    let event: Event = serde_json::from_str(&json)?;

    // Verify signature
    if !event.verify().is_ok() {
        bail!("Invalid nostr event, invalid signature");
    }

    // Check event kind (NIP-98: HTTP Auth, kind 27235)
    if event.kind != Kind::Custom(27235) {
        bail!("Invalid nostr event, wrong kind");
    }

    // Check timestamp (within 120 seconds)
    let now = Utc::now();
    let event_time = DateTime::from_timestamp(event.created_at.as_u64() as i64, 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
    let diff_seconds = (now - event_time).num_seconds().abs();
    if diff_seconds > 120 {
        bail!("Invalid nostr event, timestamp out of range");
    }

    // Check URL tag (full URI)
    let url_tag = event
        .tags
        .iter()
        .find(|tag| tag.kind() == TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::U)))
        .and_then(|tag| tag.content())
        .ok_or_else(|| anyhow::anyhow!("Missing URL tag"))?;

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

    if !url_tag.eq_ignore_ascii_case(&request_uri) {
        bail!(
            "Invalid nostr event, URL tag invalid. Expected: {}, Got: {}",
            request_uri,
            url_tag
        );
    }

    // Check method tag
    let method_tag = event
        .tags
        .iter()
        .find(|tag| tag.kind() == TagKind::Method)
        .and_then(|tag| tag.content())
        .ok_or_else(|| anyhow::anyhow!("Missing method tag"))?;

    if !method_tag.eq_ignore_ascii_case(req.method().as_str()) {
        bail!("Invalid nostr event, method tag invalid");
    }

    Ok(AuthResult {
        pubkey: event.pubkey.clone(),
        event,
    })
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
