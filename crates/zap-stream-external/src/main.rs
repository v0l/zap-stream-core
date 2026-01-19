#[cfg(feature = "cloudflare")]
use crate::cloudflare::{CfApiWrapper, CloudflareToken};
use anyhow::Result;
use axum::Router;
use config::Config;
use nostr_sdk::Keys;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use zap_stream::admin_api::ZapStreamAdminApiImpl;
use zap_stream::http::IndexRouter;
use zap_stream::payments::{PaymentBackend, PaymentHandler, create_lightning};
use zap_stream::setup_crypto_provider;
use zap_stream::stream_manager::StreamManager;
use zap_stream_api_common::AxumAdminApi;
use zap_stream_db::ZapStreamDb;

#[cfg(feature = "cloudflare")]
mod cloudflare;

#[derive(Clone, Deserialize)]
struct Settings {
    /// Database connection string (mysql://)
    database: String,

    /// Bind address for HTTP server
    listen_http: String,

    /// Public URL which points to this http server
    public_url: String,

    /// Payment backend config
    payments: PaymentBackend,

    /// Nostr NSEC for publishing nostr events
    nsec: String,

    #[serde(default)]
    /// List of nostr relays to connect to and publish events
    relays: Vec<String>,

    #[cfg(feature = "cloudflare")]
    /// API details for cloudflare backend
    cloudflare: CloudflareToken,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    setup_crypto_provider();

    info!("Starting zap-stream-cf");
    let mut tasks = vec![];

    let mut builder = Config::builder()
        .add_source(config::File::with_name("config.yaml"))
        .add_source(config::Environment::with_prefix("APP").separator("__"));
    #[cfg(debug_assertions)]
    {
        builder = builder.add_source(config::File::with_name("config.dev.yaml").required(false));
    }
    let settings: Settings = builder.build()?.try_deserialize()?;

    // setup termination handler
    let shutdown = CancellationToken::new();

    // setup handler for clean shutdown
    let shutdown_sig = shutdown.clone();
    ctrlc::set_handler(move || {
        info!("Shutdown requested!");
        shutdown_sig.cancel();
    })
    .expect("Error setting Ctrl-C handler");

    // setup database
    let db = ZapStreamDb::new(&settings.database).await?;
    db.migrate().await?;

    // setup nostr client
    let keys = Keys::from_str(&settings.nsec)?;
    let client = nostr_sdk::ClientBuilder::new().signer(keys).build();
    for r in &settings.relays {
        client.add_relay(r).await?;
    }
    client.connect().await;

    // connect lightning node
    let node = create_lightning(&settings.payments, db.clone()).await?;

    // start payment handler
    let handler = PaymentHandler::new(node.clone(), db.clone(), client.clone());
    tasks.push(handler.start_payment_handler(shutdown.clone()));

    // create stream manager to track active stream info
    let stream_manager = StreamManager::new("zap-stream-cf".to_string());

    // start http server
    let admin_api_impl =
        ZapStreamAdminApiImpl::new(db.clone(), PathBuf::new(), Vec::new(), "".to_string());
    let http_addr: SocketAddr = settings.listen_http.parse()?;
    #[allow(unused_mut)]
    let mut server = Router::new()
        .merge(IndexRouter::new(stream_manager.clone()))
        .merge(AxumAdminApi::new(admin_api_impl));

    #[cfg(feature = "cloudflare")]
    {
        use zap_stream::http::ZapRouter;
        use zap_stream_api_common::AxumApi;
        let api_impl = CfApiWrapper::new(
            settings.cloudflare,
            db.clone(),
            client.clone(),
            node.clone(),
        );
        server = server
            .merge(AxumApi::new(api_impl.clone()))
            .merge(api_impl.make_router())
            .merge(ZapRouter::new(
                settings.public_url.clone(),
                client.clone(),
                db.clone(),
                api_impl.clone(),
            ));
    }
    let shutdown_http = shutdown.clone();
    tasks.push(tokio::spawn(async move {
        let listener = TcpListener::bind(&http_addr).await?;
        info!("Listening on: {}", http_addr);
        axum::serve(listener, server.layer(CorsLayer::very_permissive()))
            .with_graceful_shutdown(async move { shutdown_http.cancelled().await })
            .await?;
        info!("HTTP server shutdown.");
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
