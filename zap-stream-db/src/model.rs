use chrono::{DateTime, Utc};
use sqlx::{FromRow, Type};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: u64,
    pub pubkey: [u8; 32],
    pub created: DateTime<Utc>,
    pub balance: i64,
    pub tos_accepted: DateTime<Utc>,
    pub stream_key: String,
    pub is_admin: bool,
    pub is_blocked: bool,
}

#[derive(Default, Debug, Clone, Type)]
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
    pub id: u64,
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
}