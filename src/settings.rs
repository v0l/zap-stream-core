use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// List of listen endpoints
    ///
    /// - srt://localhost:3333
    /// - tcp://localhost:3334
    /// - rtmp://localhost:1935
    pub endpoints: Vec<String>,

    /// Where to store output (static files)
    pub output_dir: String,

    /// Public facing URL that maps to [output_dir]
    pub public_url: String,

    /// Binding address for http server serving files from [output_dir]
    pub listen_http: String,

    /// Overseer service see [crate::overseer::Overseer] for more info
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
        /// Cost (milli-sats) / second / variant
        cost: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LndSettings {
    pub address: String,
    pub cert: String,
    pub macaroon: String,
}
