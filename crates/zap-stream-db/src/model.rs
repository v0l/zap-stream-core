use chrono::{DateTime, Utc};
use sqlx::{FromRow, Type};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    /// Database ID for this uer
    pub id: u64,
    /// Nostr pubkey of this user
    pub pubkey: Vec<u8>,
    /// Timestamp when this user first used the service
    pub created: DateTime<Utc>,
    /// Current balance in milli-sats
    pub balance: i64,
    /// When the TOS was accepted
    pub tos_accepted: Option<DateTime<Utc>>,
    /// Primary stream key
    pub stream_key: String,
    /// If the user is an admin
    pub is_admin: bool,
    /// If the user is blocked from streaming
    pub is_blocked: bool,
    /// Streams are recorded
    pub recording: bool,
    /// Stream dump recording is enabled
    pub stream_dump_recording: bool,
    /// Default stream title
    pub title: Option<String>,
    /// Default stream summary
    pub summary: Option<String>,
    /// Default stream image
    pub image: Option<String>,
    /// Default tags (comma separated)
    pub tags: Option<String>,
    /// Default content warning
    pub content_warning: Option<String>,
    /// Default stream goal
    pub goal: Option<String>,
    /// Nostr Wallet Connect configuration
    pub nwc: Option<String>,
    /// Users selected default ingest ID
    pub ingest_id: Option<u64>,
}

#[derive(Default, Debug, Clone, PartialEq, Type)]
#[repr(u8)]
pub enum UserStreamState {
    #[default]
    Unknown = 0,
    Planned = 1,
    Live = 2,
    Ended = 3,
}

impl Display for UserStreamState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UserStreamState::Unknown => write!(f, "unknown"),
            UserStreamState::Planned => write!(f, "planned"),
            UserStreamState::Live => write!(f, "live"),
            UserStreamState::Ended => write!(f, "ended"),
        }
    }
}

/// A stream event belonging to a user
#[derive(Debug, Clone, Default, FromRow)]
pub struct UserStream {
    /// Unique stream id (UUID)
    pub id: String,
    /// The user who owns this stream
    pub user_id: u64,
    /// Timestamp when the stream started or starts when planned
    pub starts: DateTime<Utc>,
    /// Timestamp when the stream ended, or ends when planned
    pub ends: Option<DateTime<Utc>>,
    /// Current state of the stream live/ended/planned
    pub state: UserStreamState,
    /// Stream title
    pub title: Option<String>,
    /// Stream summary long description
    pub summary: Option<String>,
    /// Poster image URL
    pub image: Option<String>,
    /// Thumbnail image URL
    pub thumb: Option<String>,
    /// Comma-seperated list of hashtags
    pub tags: Option<String>,
    /// Content warning tag
    pub content_warning: Option<String>,
    /// Stream zap goal event ID
    pub goal: Option<String>,
    /// Pinned comment event ID
    pub pinned: Option<String>,
    /// Total cost in milli-sats
    pub cost: u64,
    /// Total stream duration in seconds
    pub duration: f32,
    /// Entry fee to be paid by viewers
    pub fee: Option<u32>,
    /// The raw NOSTR event json for this stream
    pub event: Option<String>,
    /// The ingest endpoint id
    pub endpoint_id: Option<u64>,
    /// The node hostname running this stream
    pub node_name: Option<String>,
    /// Fixed key ID used for this stream event
    pub stream_key_id: Option<u64>,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserStreamForward {
    pub id: u64,
    /// Owner user id
    pub user_id: u64,
    /// User designated label
    pub name: String,
    /// Target RTMP url for forwarding
    pub target: String,
    /// Whether this forward is disabled
    pub disabled: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserStreamKey {
    pub id: u64,
    /// The owner user id
    pub user_id: u64,
    /// The stream key (UUID)
    pub key: String,
    /// Timestamp when the key was created
    pub created: DateTime<Utc>,
    /// Expiration timestamp for this stream key
    pub expires: Option<DateTime<Utc>>,
    /// Fixed user stream this key references (UUID)
    pub stream_id: String,
}

#[derive(Default, Debug, Clone, Type)]
#[repr(u8)]
pub enum PaymentType {
    #[default]
    TopUp = 0,
    Zap = 1,
    Credit = 2,
    Withdrawal = 3,
    AdmissionFee = 4,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamKeyType {
    Primary(u64),
    FixedEventKey { id: u64, stream_id: String },
}

impl StreamKeyType {
    pub fn user_id(&self) -> u64 {
        match self {
            StreamKeyType::Primary(id) => *id,
            StreamKeyType::FixedEventKey { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct Payment {
    pub payment_hash: Vec<u8>,
    pub user_id: u64,
    pub invoice: Option<String>,
    pub is_paid: bool,
    pub amount: i64,
    pub created: DateTime<Utc>,
    pub expires: DateTime<Utc>,
    pub nostr: Option<String>,
    pub payment_type: PaymentType,
    pub fee: u64,
    pub external_data: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct IngestEndpoint {
    pub id: u64,
    pub name: String,
    pub cost: u64,
    pub capabilities: Option<String>, // JSON array stored as string
}

#[derive(Debug, Clone, FromRow)]
pub struct AuditLog {
    pub id: u64,
    pub admin_id: u64,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub message: String,
    pub metadata: Option<Vec<u8>>, // JSON stored as BLOB
    pub created: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AuditLogWithPubkeys {
    pub id: u64,
    pub admin_id: u64,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub message: String,
    pub metadata: Option<Vec<u8>>, // JSON stored as BLOB
    pub created: DateTime<Utc>,
    pub admin_pubkey: Vec<u8>,
    pub target_pubkey: Option<Vec<u8>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserHistoryEntry {
    pub created: DateTime<Utc>,
    pub amount: u64,
    pub payment_type: Option<u8>, // Payment type for payments, None for streams
    pub nostr: Option<String>,    // Nostr content for zaps
    pub stream_title: Option<String>, // Stream title for stream entries
    pub stream_id: Option<String>, // Stream ID for stream entries
}

#[derive(Debug, Clone, FromRow)]
pub struct UserPreviousStreams {
    /// Number of live streams using primary key (stream_key_id is null)
    pub live_primary_count: i64,
    /// Number of live streams using stream key (stream_key_id is not null)
    pub live_stream_key_count: i64,
    /// Timestamp when the last primary key stream ended
    pub last_ended: Option<DateTime<Utc>>,
    /// ID of the last primary key stream that ended
    pub last_stream_id: Option<String>,
}
