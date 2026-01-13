use serde::{Deserialize, Serialize};
use zap_stream_api_common::TwitchConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// List of listen endpoints
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

    #[serde(default)]
    /// Ignore the url check inNIP-98
    pub ignore_auth_url: Option<bool>,

    /// Binding address for http server serving files from [output_dir]
    pub listen_http: String,

    #[serde(default)]
    /// Admin pubkey
    pub admin_pubkey: Option<String>,

    /// Overseer service see [Overseer] for more info
    pub overseer: OverseerConfig,

    #[serde(default)]
    /// Redis config for horizonal-scaling
    pub redis: Option<RedisConfig>,

    #[cfg(feature = "moq")]
    /// MoQ server config
    pub moq: Option<moq_native::ServerConfig>,

    #[serde(default)]
    /// Twitch API configuration
    pub twitch: TwitchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct OverseerConfig {
    /// MySQL connection string
    pub database: String,
    /// Backend payment target
    pub payments: PaymentBackend,
    /// Relays to publish events to
    pub relays: Vec<String>,
    /// Nsec to sign nostr events
    pub nsec: String,
    /// Blossom servers
    pub blossom: Option<Vec<String>>,
    /// Segment length for HLS egress
    pub segment_length: Option<f32>,
    /// Low balance notification threshold in sats
    pub low_balance_threshold: Option<u64>,
    /// Advertise this server on nostr for others to use (NIP-89)
    pub advertise: Option<AdvertiseConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvertiseConfig {
    /// Metadata name
    pub name: Option<String>,
    /// Metadata about
    pub about: Option<String>,
    /// Metadata picture
    pub picture: Option<String>,
    /// Optional override for the 'd' tag
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentBackend {
    #[serde(rename_all = "kebab-case")]
    LND {
        address: String,
        cert: String,
        macaroon: String,
    },
    #[serde(rename_all = "kebab-case")]
    Bitvora {
        api_token: String,
        webhook_secret: String,
    },
    #[serde(rename_all = "kebab-case")]
    NWC { url: String },
    #[serde(rename_all = "kebab-case")]
    // Plain LUD-16 payment backend
    LNURL { address: String },
}
