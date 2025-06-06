use crate::api::Api;
use anyhow::{bail, Result};
use base64::Engine;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::Service;
use hyper::{Method, Request, Response};
use log::error;
use nostr_sdk::{serde_json, Alphabet, Event, Kind, PublicKey, SingleLetterTag, TagKind};
use serde::{Serialize, Deserialize};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::File;  
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use zap_stream_core::viewer::ViewerTracker;

#[derive(Serialize)]
struct StreamData {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    live_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_count: Option<u64>,
}

#[derive(Serialize)]
struct IndexTemplateData {
    public_url: String,
    has_streams: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    streams: Vec<StreamData>,
}

struct CachedStreams {
    data: IndexTemplateData,
    cached_at: Instant,
}

pub type StreamCache = Arc<RwLock<Option<CachedStreams>>>;

#[derive(Clone)]
pub struct HttpServer {
    index_template: String,
    files_dir: PathBuf,
    api: Api,
    stream_cache: StreamCache,
}

impl HttpServer {
    pub fn new(index_template: String, files_dir: PathBuf, api: Api, stream_cache: StreamCache) -> Self {
        Self {
            index_template,
            files_dir,
            api,
            stream_cache,
        }
    }

    async fn get_cached_or_fetch_streams(&self) -> Result<IndexTemplateData> {
        Self::get_cached_or_fetch_streams_static(&self.stream_cache, &self.api).await
    }

    async fn get_cached_or_fetch_streams_static(stream_cache: &StreamCache, api: &Api) -> Result<IndexTemplateData> {
        const CACHE_DURATION: Duration = Duration::from_secs(60); // 1 minute

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
                    StreamData {
                        id: stream.id.clone(),
                        title: stream.title.unwrap_or_else(|| format!("Stream {}", &stream.id[..8])),
                        summary: stream.summary,
                        live_url: format!("/{}/live.m3u8", stream.id),
                        viewer_count: if viewer_count > 0 { Some(viewer_count) } else { None },
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

    async fn render_index(&self) -> Result<String> {
        let template_data = self.get_cached_or_fetch_streams().await?;
        let template = mustache::compile_str(&self.index_template)?;
        let rendered = template.render_to_string(&template_data)?;
        Ok(rendered)
    }

    async fn handle_hls_playlist(
        api: &Api,
        req: &Request<Incoming>,
        playlist_path: &PathBuf,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        // Extract stream ID from path (e.g., /uuid/live.m3u8 -> uuid)
        let path_parts: Vec<&str> = req.uri().path().trim_start_matches('/').split('/').collect();
        if path_parts.len() < 2 {
            return Ok(Response::builder().status(404).body(BoxBody::default())?);
        }
        
        let stream_id = path_parts[0];
        
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
        let modified_content = Self::add_viewer_token_to_playlist(&playlist_content, &viewer_token)?;

        Ok(Response::builder()
            .header("content-type", "application/vnd.apple.mpegurl")
            .header("server", "zap-stream-core")
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-headers", "*")
            .header("access-control-allow-methods", "HEAD, GET")
            .body(
                Full::new(Bytes::from(modified_content))
                    .map_err(|e| match e {})
                    .boxed(),
            )?)
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

        // Fallback to connection IP (note: in real deployment this might be a proxy)
        "unknown".to_string()
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
                master.write_to(&mut output)
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
}

impl Service<Request<Incoming>> for HttpServer {
    type Response = Response<BoxBody<Bytes, Self::Error>>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        // check is index.html
        if req.method() == Method::GET && req.uri().path() == "/"
            || req.uri().path() == "/index.html"
        {
            let stream_cache = self.stream_cache.clone();
            let api = self.api.clone();
            
            // Compile template outside async move for better performance
            let template = match mustache::compile_str(&self.index_template) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to compile template: {}", e);
                    return Box::pin(async move {
                        Ok(Response::builder()
                            .status(500)  
                            .body(BoxBody::default()).unwrap())
                    });
                }
            };
            
            return Box::pin(async move {
                // Use the existing method to get cached template data
                let template_data = Self::get_cached_or_fetch_streams_static(&stream_cache, &api).await;

                match template_data {
                    Ok(data) => {
                        match template.render_to_string(&data) {
                            Ok(index_html) => Ok(Response::builder()
                                .header("content-type", "text/html")
                                .header("server", "zap-stream-core")
                                .body(
                                    Full::new(Bytes::from(index_html))
                                        .map_err(|e| match e {})
                                        .boxed(),
                                )?),
                            Err(e) => {
                                error!("Failed to render template: {}", e);
                                Ok(Response::builder()
                                    .status(500)
                                    .body(BoxBody::default())?)
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to fetch template data: {}", e);
                        Ok(Response::builder()
                            .status(500)
                            .body(BoxBody::default())?)
                    }
                }
            });
        }

        // check if mapped to file
        let dst_path = self.files_dir.join(req.uri().path()[1..].to_string());
        if dst_path.exists() {
            let api_clone = self.api.clone();
            return Box::pin(async move {
                let rsp = Response::builder()
                    .header("server", "zap-stream-core")
                    .header("access-control-allow-origin", "*")
                    .header("access-control-allow-headers", "*")
                    .header("access-control-allow-methods", "HEAD, GET");

                if req.method() == Method::HEAD {
                    return Ok(rsp.body(BoxBody::default())?);
                }

                // Handle HLS playlists with viewer tracking
                if req.uri().path().ends_with("/live.m3u8") {
                    return Self::handle_hls_playlist(&api_clone, &req, &dst_path).await;
                }

                // Handle regular files
                let f = File::open(&dst_path).await?;
                let f_stream = ReaderStream::new(f);
                let body = StreamBody::new(
                    f_stream
                        .map_ok(Frame::data)
                        .map_err(|e| Self::Error::new(e)),
                )
                .boxed();
                Ok(rsp.body(body)?)
            });
        }

        // otherwise handle in overseer
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
        Some(query) => format!("{}{}?{}", public_url.trim_end_matches('/'), req.uri().path(), query),
        None => format!("{}{}", public_url.trim_end_matches('/'), req.uri().path()),
    };

    if !url_tag.eq_ignore_ascii_case(&request_uri) {
        bail!("Invalid nostr event, URL tag invalid. Expected: {}, Got: {}", request_uri, url_tag);
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
