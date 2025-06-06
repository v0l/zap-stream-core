use crate::overseer::ZapStreamOverseer;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// List of listen endpoints
    ///
    /// - srt://localhost:3333
    /// - tcp://localhost:3334
    /// - rtmp://localhost:1935
    pub endpoints: Vec<String>,

    /// Public facing hostname that maps to [endpoints]
    pub endpoints_public_hostname: String,

    /// Where to store output (static files)
    pub output_dir: String,

    /// Public facing URL that maps to [output_dir]
    pub public_url: String,

    /// Binding address for http server serving files from [output_dir]
    pub listen_http: String,

    /// Overseer service see [Overseer] for more info
    pub overseer: OverseerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverseerConfig {
    /// Static output
    Local,
    /// Control system via external API
    Webhook {
        /// Webhook service URL
        url: String,
    },
    /// NIP-53 service (i.e. zap.stream backend)
    ZapStream {
        /// MYSQL database connection string
        database: String,
        /// LND node connection details
        lnd: LndSettings,
        /// Relays to publish events to
        relays: Vec<String>,
        /// Nsec to sign nostr events
        nsec: String,
        /// Blossom servers
        blossom: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LndSettings {
    pub address: String,
    pub cert: String,
    pub macaroon: String,
}

impl Settings {
    pub async fn get_overseer(&self) -> anyhow::Result<Arc<ZapStreamOverseer>> {
        match &self.overseer {
            OverseerConfig::ZapStream {
                nsec: private_key,
                database,
                lnd,
                relays,
                blossom,
            } => Ok(Arc::new(
                ZapStreamOverseer::new(
                    &self.output_dir,
                    &self.public_url,
                    private_key,
                    database,
                    lnd,
                    relays,
                    blossom,
                )
                .await?,
            )),
            _ => {
                panic!("Unsupported overseer");
            }
        }
    }
}
