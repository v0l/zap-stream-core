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

    /// Terms of Service URL (defaults to https://zap.stream/tos)
    #[serde(default)]
    tos_url: Option<String>,

    /// Client URL for "Watch live on" alt tag (defaults to https://zap.stream)
    #[serde(default)]
    client_url: Option<String>,

    /// Public hostname for ingest endpoints (custom RTMPS domain)
    #[serde(default)]
    endpoints_public_hostname: Option<String>,

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

/// Environment-variable configuration source.
///
/// `APP__` prefix, `__` as the nesting separator (so `APP__CLOUDFLARE__TOKEN`
/// sets `cloudflare.token`). Added after the config file, so environment
/// variables override file values.
///
/// `try_parsing` + `list_separator` let list-valued keys be supplied as a
/// comma-separated string, e.g. `APP__RELAYS=wss://a,wss://b`. Without both of
/// these a list key fails with "invalid type: string ..., expected a sequence".
/// Normalise a list of relay URLs coming from configuration.
///
/// Comma-separated environment variables are edited by hand (often in a web
/// form), so entries routinely arrive with surrounding whitespace, and a
/// trailing comma yields an empty entry. Neither is rejected by the config
/// crate: `APP__RELAYS="wss://a, wss://b,"` deserializes to
/// `["wss://a", " wss://b", ""]`, whose last two entries silently fail to
/// connect. Trim each entry and drop the empties so a stray keystroke cannot
/// quietly cost a relay.
fn normalise_relays(relays: &[String]) -> Vec<String> {
    relays
        .iter()
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .map(|r| r.to_string())
        .collect()
}

fn env_source() -> config::Environment {
    config::Environment::with_prefix("APP")
        .separator("__")
        .try_parsing(true)
        .list_separator(",")
        .with_list_parse_key("relays")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    setup_crypto_provider();

    info!("Starting zap-stream-cf");
    let mut tasks = vec![];

    let mut builder = Config::builder()
        .add_source(config::File::with_name("config.yaml"))
        .add_source(env_source());
    #[cfg(debug_assertions)]
    {
        builder = builder.add_source(config::File::with_name("config.dev.yaml").required(false));
    }
    let mut settings: Settings = builder.build()?.try_deserialize()?;
    settings.relays = normalise_relays(&settings.relays);

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
            stream_manager.clone(),
            settings.public_url.clone(),
            settings.endpoints_public_hostname.clone(),
            settings.tos_url.clone(),
            settings.client_url.clone(),
        )
        .await?;
        api_impl.setup_webhook().await?;
        // Notification policy setup runs as a background task because Cloudflare
        // validates the webhook URL on destination creation — the HTTP server
        // must be listening before this can succeed.
        let api_for_notify = api_impl.clone();
        tasks.push(tokio::spawn(async move {
            // Brief delay to let the HTTP server bind
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Err(e) = api_for_notify.setup_notification_policy().await {
                tracing::error!("Failed to setup notification policy: {}", e);
            }
            Ok(())
        }));
        tasks.push(api_impl.clone().check_streams(shutdown.clone()));
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

#[cfg(test)]
mod tests {
    use super::{Settings, env_source, normalise_relays};

    /// Minimal valid config file contents, mirroring the shipped config.yaml shape.
    fn base_yaml() -> String {
        r#"
database: "mysql://root:root@localhost:3306/zap_stream"
listen_http: "[::]:8080"
public_url: "https://file.example"
tos_url: "https://file.example/tos"
client_url: "https://file.example/live"
nsec: "nsec1wya428srvpu96n4h78gualaj7wqw4ecgatgja8d5ytdqrxw56r2se440y4"
relays:
  - "wss://file-one"
  - "wss://file-two"
payments:
  lnurl:
    address: "file@example.com"
cloudflare:
  token: "file-token"
  account_id: "file-account"
"#
        .to_string()
    }

    fn settings_with(env: &[(&str, &str)]) -> Result<Settings, config::ConfigError> {
        let dir = std::env::temp_dir().join(format!("zsx-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("c{}.yaml", env.len()));
        std::fs::write(&path, base_yaml()).unwrap();
        let map: config::Map<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        config::Config::builder()
            .add_source(config::File::from(path.as_path()))
            .add_source(env_source().source(Some(map)))
            .build()?
            .try_deserialize()
    }

    #[test]
    fn env_overrides_file_scalar() {
        let s = settings_with(&[("APP__TOS_URL", "https://env.example/tos")]).unwrap();
        assert_eq!(s.tos_url.as_deref(), Some("https://env.example/tos"));
    }

    #[test]
    fn env_overrides_nested_scalar() {
        let s = settings_with(&[("APP__PAYMENTS__LNURL__ADDRESS", "env@example.com")]).unwrap();
        assert!(format!("{:?}", s.payments).contains("env@example.com"));
    }

    #[test]
    fn relays_parse_from_comma_separated_env() {
        let s = settings_with(&[("APP__RELAYS", "wss://env-one,wss://env-two,wss://env-three")]).unwrap();
        assert_eq!(s.relays, vec!["wss://env-one", "wss://env-two", "wss://env-three"]);
    }

    #[test]
    fn relays_fall_back_to_file_when_env_absent() {
        let s = settings_with(&[]).unwrap();
        assert_eq!(s.relays, vec!["wss://file-one", "wss://file-two"]);
    }

    /// try_parsing(true) coerces env values into bool/int/float where they look
    /// like one. Guard that a numeric-looking value destined for a String field
    /// still deserializes, since several Settings fields are plain Strings.
    #[test]
    fn numeric_looking_value_for_string_field() {
        let r = settings_with(&[("APP__PUBLIC_URL", "8080")]);
        match r {
            Ok(s) => assert_eq!(s.public_url, "8080"),
            Err(e) => panic!("try_parsing broke a String field: {e}"),
        }
    }

    #[test]
    fn normalise_relays_trims_and_drops_empties() {
        let input: Vec<String> = ["wss://a", " wss://b", "", "  ", "wss://c  "]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(normalise_relays(&input), vec!["wss://a", "wss://b", "wss://c"]);
    }

    #[test]
    fn normalise_relays_handles_env_style_mistakes() {
        // trailing comma -> empty final entry
        assert_eq!(normalise_relays(&["wss://a".into(), "wss://b".into(), "".into()]),
                   vec!["wss://a", "wss://b"]);
        // space after comma -> leading whitespace
        assert_eq!(normalise_relays(&["wss://a".into(), " wss://b".into()]),
                   vec!["wss://a", "wss://b"]);
        // entirely empty value
        assert!(normalise_relays(&["".into()]).is_empty());
    }

    #[test]
    fn normalise_relays_leaves_clean_input_untouched() {
        let input: Vec<String> = ["wss://a".into(), "wss://b".into()].into();
        assert_eq!(normalise_relays(&input), input);
    }
}
