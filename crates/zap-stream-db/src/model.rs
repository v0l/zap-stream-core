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

#[derive(Debug, Clone, Default, FromRow)]
pub struct UserStream {
    pub id: String,
    pub user_id: u64,
    pub starts: DateTime<Utc>,
    pub ends: Option<DateTime<Utc>>,
    pub state: UserStreamState,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub thumb: Option<String>,
    pub tags: Option<String>,
    pub content_warning: Option<String>,
    pub goal: Option<String>,
    pub pinned: Option<String>,
    pub cost: u64,
    pub duration: f32,
    pub fee: Option<u32>,
    pub event: Option<String>,
    pub endpoint_id: Option<u64>,
    pub last_segment: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserStreamForward {
    pub id: u64,
    pub user_id: u64,
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct UserStreamKey {
    pub id: u64,
    pub user_id: u64,
    pub key: String,
    pub created: DateTime<Utc>,
    pub expires: Option<DateTime<Utc>>,
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

#[derive(Debug, Clone, FromRow)]
pub struct Payment {
    pub payment_hash: Vec<u8>,
    pub user_id: u64,
    pub invoice: Option<String>,
    pub is_paid: bool,
    pub amount: u64,
    pub created: DateTime<Utc>,
    pub nostr: Option<String>,
    pub payment_type: PaymentType,
    pub fee: u64,
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
    pub metadata: Option<String>, // JSON stored as string
    pub created: DateTime<Utc>,
}
