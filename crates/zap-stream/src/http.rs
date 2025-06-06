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
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

#[derive(Clone)]
pub struct HttpServer {
    index: String,
    files_dir: PathBuf,
    api: Api,
}

impl HttpServer {
    pub fn new(index: String, files_dir: PathBuf, api: Api) -> Self {
        Self {
            index,
            files_dir,
            api,
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
            let index = self.index.clone();
            return Box::pin(async move {
                Ok(Response::builder()
                    .header("content-type", "text/html")
                    .header("server", "zap-stream-core")
                    .body(
                        Full::new(Bytes::from(index))
                            .map_err(|e| match e {})
                            .boxed(),
                    )?)
            });
        }

        // check if mapped to file
        let dst_path = self.files_dir.join(req.uri().path()[1..].to_string());
        if dst_path.exists() {
            return Box::pin(async move {
                let rsp = Response::builder()
                    .header("server", "zap-stream-core")
                    .header("access-control-allow-origin", "*")
                    .header("access-control-allow-headers", "*")
                    .header("access-control-allow-methods", "HEAD, GET");

                if req.method() == Method::HEAD {
                    return Ok(rsp.body(BoxBody::default())?);
                }
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
