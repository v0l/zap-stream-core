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
use ffmpeg_rs_raw::{Decoder, ffmpeg_sys_the_third, rstr};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use payments_rs::lightning::setup_crypto_provider;
use std::io::stdout;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};
use zap_stream_core::listen::try_create_listener;
use zap_stream_core::metrics::PipelineMetrics;
use zap_stream_core::overseer::Overseer;

mod api;
mod auth;
mod http;
mod multitrack;
mod overseer;
mod payments;
mod settings;
mod stream_manager;
mod viewer;
mod websocket_metrics;
mod game_db;

#[derive(Parser, Debug)]
#[clap(version, about)]
struct Args {}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "aarch64")))]
type VaList = ffmpeg_sys_the_third::va_list;
#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
type VaList = *mut ffmpeg_sys_the_third::__va_list_tag;
#[cfg(target_os = "android")]
type VaList = [u64; 4];

#[unsafe(no_mangle)]
pub unsafe extern "C" fn av_log_redirect(
    av_class: *mut libc::c_void,
    level: libc::c_int,
    fmt: *const libc::c_char,
    args: VaList,
) {
    unsafe {
        use ffmpeg_sys_the_third::*;
        let mut buf: [u8; 1024] = [0; 1024];
        let mut prefix: libc::c_int = 1;
        av_log_format_line(
            av_class,
            level,
            fmt,
            args,
            buf.as_mut_ptr() as *mut libc::c_char,
            1024,
            ptr::addr_of_mut!(prefix),
        );
        // Find the null terminator to avoid logging trailing null bytes
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let msg = String::from_utf8_lossy(&buf[..len]).trim_end().to_string();
        match level {
            AV_LOG_DEBUG => {
                tracing::debug!(target: "ffmpeg", "{}", msg)
            }
            AV_LOG_WARNING => {
                tracing::warn!(target: "ffmpeg", "{}", msg)
            }
            AV_LOG_INFO => {
                tracing::info!(target: "ffmpeg", "{}", msg)
            }
            AV_LOG_ERROR | AV_LOG_PANIC | AV_LOG_FATAL => {
                tracing::error!(target: "ffmpeg", "{}", msg)
            }
            _ => tracing::trace!(target: "ffmpeg", "{}", msg),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _args = Args::parse();

    let logger = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::Layer::new()
            .with_writer(stdout)
            .with_filter(EnvFilter::from_default_env()),
    );
    tracing::subscriber::set_global_default(logger)?;

    info!("Starting zap-stream");

    // Initialize prometheus metrics
    if let Err(e) = PipelineMetrics::init_global() {
        warn!("Failed to initialize pipeline metrics: {}", e);
    }

    setup_crypto_provider();

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

    #[cfg(feature = "moq")]
    let moq_origin = Arc::new(zap_stream_core::hang::moq_lite::Origin::produce());

    let settings: Settings = builder.try_deserialize()?;
    let (overseer, api) = {
        let mut overseer = ZapStreamOverseer::from_settings(&settings, shutdown.clone()).await?;
        #[cfg(feature = "moq")]
        if let Some(ref ms) = settings.moq {
            overseer.set_moq_origin(moq_origin.clone(), ms.bind.clone());
        }

        let arc = Arc::new(overseer);
        let api = Api::new(arc.clone(), settings.clone());
        (arc, api)
    };
    let mut tasks = vec![];

    //listen for invoice
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

    // QUIC server
    #[cfg(feature = "moq")]
    if let Some(cfg) = settings.moq {
        let mut server = cfg.init()?;
        tasks.push(tokio::spawn(async move {
            info!("MoQ server started..");

            while let Some(req) = server.accept().await {
                let session = match req.ok().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Failed to accept QUIC/WebTransport session {}", e);
                        continue;
                    }
                };

                match zap_stream_core::hang::moq_lite::Session::accept(
                    session,
                    moq_origin.producer.consume(),
                    moq_origin.producer.clone(),
                )
                .await
                {
                    Ok(session) => {
                        tokio::spawn(async move {
                            if let Err(e) = session.closed().await {
                                error!("MoQ session closed with error {}", e);
                            }
                            info!("MoQ session closed.");
                        });
                    }
                    Err(e) => {
                        error!("Failed to create MoQ session {}", e);
                    }
                }
            }
            Ok(())
        }));
    }

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
