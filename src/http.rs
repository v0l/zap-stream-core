use crate::overseer::Overseer;
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::Service;
use hyper::{Method, Request, Response};
use log::error;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

#[derive(Clone)]
pub struct HttpServer {
    index: String,
    files_dir: PathBuf,
    overseer: Arc<dyn Overseer>,
}

impl HttpServer {
    pub fn new(index: String, files_dir: PathBuf, overseer: Arc<dyn Overseer>) -> Self {
        Self {
            index,
            files_dir,
            overseer,
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
        let overseer = self.overseer.clone();
        Box::pin(async move {
            match overseer.api(req).await {
                Ok(res) => Ok(res),
                Err(e) => {
                    error!("{}", e);
                    Ok(Response::builder().status(500).body(BoxBody::default())?)
                }
            }
        })
    }
}
