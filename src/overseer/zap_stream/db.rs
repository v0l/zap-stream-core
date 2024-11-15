use anyhow::Result;
use chrono::{DateTime, Utc};
use nostr_sdk::{Client, Event, EventBuilder, Kind, Tag};
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

impl UserStream {
    pub(crate) fn to_event_builder(&self) -> Result<EventBuilder> {
        let mut tags = vec![
            Tag::parse(&["d".to_string(), self.id.to_string()])?,
            Tag::parse(&["status".to_string(), self.state.to_string()])?,
            Tag::parse(&["starts".to_string(), self.starts.timestamp().to_string()])?,
        ];
        if let Some(ref ends) = self.ends {
            tags.push(Tag::parse(&[
                "ends".to_string(),
                ends.timestamp().to_string(),
            ])?);
        }
        if let Some(ref title) = self.title {
            tags.push(Tag::parse(&["title".to_string(), title.to_string()])?);
        }
        if let Some(ref summary) = self.summary {
            tags.push(Tag::parse(&["summary".to_string(), summary.to_string()])?);
        }
        if let Some(ref image) = self.image {
            tags.push(Tag::parse(&["image".to_string(), image.to_string()])?);
        }
        if let Some(ref thumb) = self.thumb {
            tags.push(Tag::parse(&["thumb".to_string(), thumb.to_string()])?);
        }
        if let Some(ref content_warning) = self.content_warning {
            tags.push(Tag::parse(&[
                "content_warning".to_string(),
                content_warning.to_string(),
            ])?);
        }
        if let Some(ref goal) = self.goal {
            tags.push(Tag::parse(&["goal".to_string(), goal.to_string()])?);
        }
        if let Some(ref pinned) = self.pinned {
            tags.push(Tag::parse(&["pinned".to_string(), pinned.to_string()])?);
        }
        if let Some(ref tags_csv) = self.tags {
            for tag in tags_csv.split(',') {
                tags.push(Tag::parse(&["t".to_string(), tag.to_string()])?);
            }
        }
        Ok(EventBuilder::new(Kind::from(30_313), "", tags))
    }

    pub(super) async fn publish_stream_event(&self, client: &Client) -> Result<Event> {
        let ev = self
            .to_event_builder()?
            .sign(&client.signer().await?)
            .await?;
        client.send_event(ev.clone()).await?;
        Ok(ev)
    }
}
