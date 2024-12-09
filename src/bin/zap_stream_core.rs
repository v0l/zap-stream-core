use anyhow::{bail, Result};
use clap::Parser;
use config::Config;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{av_log_set_callback, av_version_info};
use ffmpeg_rs_raw::{av_log_redirect, rstr};
use log::{error, info};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use url::Url;
use warp::{cors, Filter};
use zap_stream_core::background::BackgroundMonitor;
#[cfg(feature = "rtmp")]
use zap_stream_core::ingress::rtmp;
#[cfg(feature = "srt")]
use zap_stream_core::ingress::srt;
#[cfg(feature = "test-pattern")]
use zap_stream_core::ingress::test;

use zap_stream_core::ingress::{file, tcp};
use zap_stream_core::overseer::Overseer;
use zap_stream_core::settings::Settings;

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

    let mut tasks = vec![];
    for e in &settings.endpoints {
        match try_create_listener(e, &settings.output_dir, &overseer) {
            Ok(l) => tasks.push(l),
            Err(e) => error!("{}", e),
        }
    }

    let http_addr: SocketAddr = settings.listen_http.parse()?;
    let http_dir = settings.output_dir.clone();
    let index_html = include_str!("../index.html").replace("%%PUBLIC_URL%%", &settings.public_url);

    tasks.push(tokio::spawn(async move {
        let cors = cors().allow_any_origin().allow_methods(vec!["GET"]);

        let index_handle = warp::get()
            .or(warp::path("index.html"))
            .and(warp::path::end())
            .map(move |_| warp::reply::html(index_html.clone()));

        let dir_handle = warp::get().and(warp::fs::dir(http_dir)).with(cors);

        warp::serve(index_handle.or(dir_handle))
            .run(http_addr)
            .await;
        Ok(())
    }));

    // spawn background job
    let mut bg = BackgroundMonitor::new(overseer.clone());
    tasks.push(tokio::spawn(async move {
        loop {
            if let Err(e) = bg.check().await {
                error!("{}", e);
            }
            sleep(Duration::from_secs(10)).await;
        }
    }));

    for handle in tasks {
        if let Err(e) = handle.await? {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}

fn try_create_listener(
    u: &str,
    out_dir: &str,
    overseer: &Arc<dyn Overseer>,
) -> Result<JoinHandle<Result<()>>> {
    let url: Url = u.parse()?;
    match url.scheme() {
        #[cfg(feature = "srt")]
        "srt" => Ok(tokio::spawn(srt::listen(
            out_dir.to_string(),
            format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            overseer.clone(),
        ))),
        #[cfg(feature = "srt")]
        "rtmp" => Ok(tokio::spawn(rtmp::listen(
            out_dir.to_string(),
            format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            overseer.clone(),
        ))),
        "tcp" => Ok(tokio::spawn(tcp::listen(
            out_dir.to_string(),
            format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            overseer.clone(),
        ))),
        "file" => Ok(tokio::spawn(file::listen(
            out_dir.to_string(),
            PathBuf::from(url.path()),
            overseer.clone(),
        ))),
        #[cfg(feature = "test-pattern")]
        "test-pattern" => Ok(tokio::spawn(test::listen(
            out_dir.to_string(),
            overseer.clone(),
        ))),
        _ => {
            bail!("Unknown endpoint config: {u}");
        }
    }
}
