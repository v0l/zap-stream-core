use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

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
pub struct LocalOverseerVariant {
    height: u16,
    bitrate: u32,
}

impl Display for LocalOverseerVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "variant:{}:{}", self.height, self.bitrate)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct OverseerConfig {
    /// MySQL connection string
    pub database: String,
    #[cfg(feature = "zap-stream")]
    /// LND node connection details
    pub lnd: LndSettings,
    /// Relays to publish events to
    pub relays: Vec<String>,
    /// Nsec to sign nostr events
    pub nsec: String,
    /// Blossom servers
    pub blossom: Option<Vec<String>>,
    /// Segment length for HLS egress
    pub segment_length: Option<f32>,
    /// Low balance notification settings
    pub low_balance_notification: Option<LowBalanceNotificationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LowBalanceNotificationConfig {
    /// Balance threshold in millisats for low balance warning
    pub threshold_msats: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LndSettings {
    pub address: String,
    pub cert: String,
    pub macaroon: String,
}
