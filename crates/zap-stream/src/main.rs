use anyhow::{bail, Result};
use clap::Parser;
use config::Config;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{av_log_set_callback, av_version_info};
use ffmpeg_rs_raw::{av_log_redirect, rstr};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use log::{error, info};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use url::Url;
#[cfg(feature = "rtmp")]
use zap_stream_core::ingress::rtmp;
#[cfg(feature = "srt")]
use zap_stream_core::ingress::srt;
#[cfg(feature = "test-pattern")]
use zap_stream_core::ingress::test;

use crate::api::Api;
use crate::http::HttpServer;
use crate::monitor::BackgroundMonitor;
use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use zap_stream_core::ingress::{file, tcp};
use zap_stream_core::overseer::Overseer;

mod api;
mod blossom;
mod http;
mod monitor;
mod overseer;
mod settings;

#[derive(Parser, Debug)]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();

    let _args = Args::parse();

    unsafe {
        av_log_set_callback(Some(av_log_redirect));
        info!("FFMPEG version={}", rstr!(av_version_info()));
    }

    let builder = Config::builder()
        .add_source(config::File::with_name("config.yaml"))
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    let settings: Settings = builder.try_deserialize()?;
    let overseer = settings.get_overseer().await?;

    // Create ingress listeners
    let mut tasks = vec![];
    for e in &settings.endpoints {
        match try_create_listener(e, &settings.output_dir, &overseer) {
            Ok(l) => tasks.push(l),
            Err(e) => error!("{}", e),
        }
    }

    let http_addr: SocketAddr = settings.listen_http.parse()?;
    let index_html = include_str!("../index.html").replace("%%PUBLIC_URL%%", &settings.public_url);

    let api = Api::new(overseer.clone(), settings.clone());
    // HTTP server
    let server = HttpServer::new(index_html, PathBuf::from(settings.output_dir), api);
    tasks.push(tokio::spawn(async move {
        let listener = TcpListener::bind(&http_addr).await?;

        loop {
            let (socket, _) = listener.accept().await?;
            let io = TokioIo::new(socket);
            let server = server.clone();
            tokio::spawn(async move {
                if let Err(e) = http1::Builder::new().serve_connection(io, server).await {
                    error!("Failed to handle request: {}", e);
                }
            });
        }
    }));

    // Background worker
    let mut bg = BackgroundMonitor::new(overseer.clone());
    tasks.push(tokio::spawn(async move {
        loop {
            if let Err(e) = bg.check().await {
                error!("{}", e);
            }
            sleep(Duration::from_secs(10)).await;
        }
    }));

    // Join tasks and get errors
    for handle in tasks {
        if let Err(e) = handle.await? {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}

pub enum ListenerEndpoint {
    SRT { endpoint: String },
    RTMP { endpoint: String },
    TCP { endpoint: String },
    File { path: PathBuf },
    TestPattern,
}

impl FromStr for ListenerEndpoint {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let url: Url = s.parse()?;
        match url.scheme() {
            "srt" => Ok(Self::SRT {
                endpoint: format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            }),
            "rtmp" => Ok(Self::RTMP {
                endpoint: format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            }),
            "tcp" => Ok(Self::TCP {
                endpoint: format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            }),
            "file" => Ok(Self::File {
                path: PathBuf::from(url.path()),
            }),
            "test-pattern" => Ok(Self::TestPattern),
            _ => bail!("Unsupported endpoint scheme: {}", url.scheme()),
        }
    }
}

fn try_create_listener(
    u: &str,
    out_dir: &str,
    overseer: &Arc<ZapStreamOverseer>,
) -> Result<JoinHandle<Result<()>>> {
    let ep = ListenerEndpoint::from_str(u)?;
    match ep {
        #[cfg(feature = "srt")]
        ListenerEndpoint::SRT { endpoint } => Ok(tokio::spawn(srt::listen(
            out_dir.to_string(),
            endpoint,
            overseer.clone(),
        ))),
        #[cfg(feature = "rtmp")]
        ListenerEndpoint::RTMP { endpoint } => Ok(tokio::spawn(rtmp::listen(
            out_dir.to_string(),
            endpoint,
            overseer.clone(),
        ))),
        ListenerEndpoint::TCP { endpoint } => Ok(tokio::spawn(tcp::listen(
            out_dir.to_string(),
            endpoint,
            overseer.clone(),
        ))),
        ListenerEndpoint::File { path } => Ok(tokio::spawn(file::listen(
            out_dir.to_string(),
            path,
            overseer.clone(),
        ))),
        #[cfg(feature = "test-pattern")]
        ListenerEndpoint::TestPattern => Ok(tokio::spawn(test::listen(
            out_dir.to_string(),
            overseer.clone(),
        ))),
        _ => {
            bail!("Unknown endpoint config: {u}");
        }
    }
}
