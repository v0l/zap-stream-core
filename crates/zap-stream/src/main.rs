use crate::api::Api;
use crate::http::HttpServer;
use crate::overseer::ZapStreamOverseer;
use crate::settings::Settings;
use anyhow::Result;
use clap::Parser;
use config::Config;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::{AV_CODEC_ID_H264, AV_CODEC_ID_HEVC};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_hwdevice_get_type_name, av_log_set_callback, av_version_info, avcodec_find_decoder,
};
use ffmpeg_rs_raw::{Decoder, av_log_redirect, rstr};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use tracing::{error, info, warn};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use zap_stream_core::listen::try_create_listener;
use zap_stream_core::overseer::Overseer;

mod api;
mod auth;
mod http;
mod overseer;
mod settings;
mod stream_manager;
mod viewer;
mod websocket_metrics;

#[derive(Parser, Debug)]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();

    let _args = Args::parse();

    info!("Starting zap-stream");

    unsafe {
        av_log_set_callback(Some(av_log_redirect));
        info!("FFMPEG version={}", rstr!(av_version_info()));

        let mut has_hw_accel = false;
        let decoder = Decoder::new();
        let h264_codec = avcodec_find_decoder(AV_CODEC_ID_H264);
        for hw in decoder.list_supported_hw_accel(h264_codec) {
            let device = av_hwdevice_get_type_name(hw);
            info!("Supported HW accel=h264_{}", rstr!(device));
            has_hw_accel = true;
        }
        let h265_codec = avcodec_find_decoder(AV_CODEC_ID_HEVC);
        for hw in decoder.list_supported_hw_accel(h265_codec) {
            let device = av_hwdevice_get_type_name(hw);
            info!("Supported HW accel=h265_{}", rstr!(device));
            has_hw_accel = true;
        }

        if !has_hw_accel {
            warn!(
                "No hardware acceleration detected, transcoding will be done entirely by the CPU!"
            );
        }
    }

    let builder = Config::builder()
        .add_source(config::File::with_name("config.yaml"))
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    // setup termination handler
    let shutdown = CancellationToken::new();

    let settings: Settings = builder.try_deserialize()?;
    let (overseer, api) = {
        let overseer = ZapStreamOverseer::from_settings(&settings, shutdown.clone()).await?;
        let arc = Arc::new(overseer);
        let api = Api::new(arc.clone(), settings.clone());
        (arc, api)
    };
    let mut tasks = vec![];

    //listen for invoice
    #[cfg(feature = "zap-stream")]
    tasks.push(overseer.start_payment_handler(shutdown.clone()));

    let shutdown_sig = shutdown.clone();
    ctrlc::set_handler(move || {
        info!("Shutdown requested!");
        shutdown_sig.cancel();
    })
    .expect("Error setting Ctrl-C handler");

    // create ingest endpoints
    let overseer = overseer as Arc<dyn Overseer>;
    for e in &settings.endpoints {
        match try_create_listener(e, &settings.output_dir, &overseer, shutdown.clone()) {
            Ok(l) => tasks.push(l),
            Err(e) => error!("{}", e),
        }
    }

    let http_addr: SocketAddr = settings.listen_http.parse()?;

    // HTTP server
    let server = HttpServer::new(PathBuf::from(settings.output_dir), api);
    let shutdown_http = shutdown.clone();
    tasks.push(tokio::spawn(async move {
        let listener = TcpListener::bind(&http_addr).await?;

        loop {
            tokio::select! {
                _ = shutdown_http.cancelled() => {
                    break;
                }
                Ok((socket, _)) = listener.accept() => {
                    let io = TokioIo::new(socket);
                    let server = server.clone();
                    let b = http1::Builder::new();

                    tokio::spawn(async move {
                        if let Err(e) = b.serve_connection(io, server).with_upgrades().await {
                            error!("Failed to handle request: {}", e);
                        }
                    });
                }
            }
        }
        info!("HTTP server shutdown.");
        Ok(())
    }));

    // Background worker to check streams
    let bg = overseer.clone();
    let shutdown_bg = shutdown.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_bg.cancelled() => {
                    break;
                }
                v = bg.check_streams() => {
                    if let Err(e) = v {
                        error!("{}", e);
                    }
                }
            }
            sleep(Duration::from_secs(10)).await;
        }
        info!("Background processor shutdown.");
        Ok(())
    }));

    // Join tasks and get errors
    for handle in tasks {
        match handle.await {
            Ok(Err(e)) => error!("{e}"),
            Err(e) => error!("{e}"),
            Ok(Ok(())) => info!("Task completed successfully."),
        }
    }
    info!("Server closed.");
    Ok(())
}
