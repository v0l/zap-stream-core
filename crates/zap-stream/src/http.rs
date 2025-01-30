use crate::api::Api;
use crate::overseer::ZapStreamOverseer;
use anyhow::{bail, Result};
use base64::Engine;
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::Service;
use hyper::{Method, Request, Response};
use log::{error, info};
use nostr_sdk::{serde_json, Event};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use zap_stream_core::overseer::Overseer;

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
        let mut dst_path = self.files_dir.join(req.uri().path()[1..].to_string());
        if dst_path.exists() {
            return Box::pin(async move {
                let mut rsp = Response::builder()
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
        let mut api = self.api.clone();
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

pub fn check_nip98_auth(req: &Request<Incoming>) -> Result<Event> {
    let auth = if let Some(a) = req.headers().get("authorization") {
        a.to_str()?
    } else {
        bail!("Authorization header missing");
    };

    if !auth.starts_with("Nostr ") {
        bail!("Invalid authorization scheme");
    }

    let json =
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(auth[6..].as_bytes())?)?;
    info!("{}", json);

    // TODO: check tags
    Ok(serde_json::from_str::<Event>(&json)?)
}
