use crate::stream_manager::StreamManager;
use anyhow::{bail, Result};
use nostr_sdk::{Client, Event, EventBuilder, JsonUtil, Kind, Tag, Timestamp};
use std::ops::Add;
use zap_stream_db::{UserStream, UserStreamState};

#[derive(Clone)]
pub struct N53Publisher {
    client: Client,
    stream_manager: StreamManager,
}

impl N53Publisher {
    pub fn new(stream_manager: StreamManager, client: Client) -> Self {
        Self {
            stream_manager,
            client,
        }
    }

    pub async fn publish(&self, ev: &Event) -> Result<()> {
        let output = self.client.send_event(ev).await?;
        if output.success.is_empty() {
            bail!("Failed to publish event: no relay accepted it");
        }
        Ok(())
    }

    pub async fn stream_to_event(
        &self,
        stream: &UserStream,
        extra_tags: Vec<Tag>,
        alt: Option<String>,
    ) -> Result<Event> {
        let mut tags = vec![
            Tag::parse(&["d".to_string(), stream.id.to_string()])?,
            Tag::parse(&["status".to_string(), stream.state.to_string()])?,
            Tag::parse(&["starts".to_string(), stream.starts.timestamp().to_string()])?,
        ];
        if let Some(ref ends) = stream.ends {
            tags.push(Tag::parse(&[
                "ends".to_string(),
                ends.timestamp().to_string(),
            ])?);
        }
        if let Some(ref title) = stream.title
            && !title.trim().is_empty()
        {
            tags.push(Tag::parse(&["title".to_string(), title.to_string()])?);
        }
        if let Some(ref summary) = stream.summary
            && !summary.trim().is_empty()
        {
            tags.push(Tag::parse(&["summary".to_string(), summary.to_string()])?);
        }
        let mut has_image = false;
        if let Some(ref image) = stream.image
            && !image.trim().is_empty()
        {
            has_image = true;
            tags.push(Tag::parse(&["image".to_string(), image.to_string()])?);
        }
        if let Some(ref thumb) = stream.thumb
            && !thumb.trim().is_empty()
        {
            if !has_image {
                tags.push(Tag::parse(&["image".to_string(), thumb.to_string()])?);
            } else {
                tags.push(Tag::parse(&["thumb".to_string(), thumb.to_string()])?);
            }
        }
        if let Some(ref content_warning) = stream.content_warning
            && !content_warning.trim().is_empty()
        {
            tags.push(Tag::parse(&[
                "content_warning".to_string(),
                content_warning.to_string(),
            ])?);
        }
        if let Some(ref goal) = stream.goal
            && !goal.trim().is_empty()
        {
            tags.push(Tag::parse(&["goal".to_string(), goal.to_string()])?);
        }
        if let Some(ref pinned) = stream.pinned
            && !pinned.trim().is_empty()
        {
            tags.push(Tag::parse(&["pinned".to_string(), pinned.to_string()])?);
        }
        if let Some(ref tags_csv) = stream.tags {
            for tag in tags_csv.split(',') {
                if tag.trim().is_empty() {
                    continue;
                }
                tags.push(Tag::parse(&["t".to_string(), tag.to_string()])?);
            }
        }

        // Add current viewer count for live streams (always from stream manager)
        if stream.state == UserStreamState::Live {
            let viewer_count = self.stream_manager.get_viewer_count(&stream.id).await;
            tags.push(Tag::parse(&[
                "current_participants".to_string(),
                viewer_count.to_string(),
            ])?);
        }

        if let Some(alt_text) = alt {
            tags.push(Tag::parse(["alt", &alt_text])?);
        }

        let mut eb = EventBuilder::new(Kind::LiveEvent, "")
            .tags(tags)
            .tags(extra_tags);

        // make sure this event is always newer
        if let Some(previous_event) = &stream.event
            && let Ok(prev_event) = Event::from_json(previous_event)
            && prev_event.created_at >= Timestamp::now()
        {
            eb = eb.custom_created_at(prev_event.created_at.add(Timestamp::from_secs(1)));
        }

        Ok(self.client.sign_event_builder(eb).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::N53Publisher;
    use crate::stream_manager::StreamManager;
    use chrono::Utc;
    use nostr_sdk::{ClientBuilder, Keys, Tag};
    use uuid::Uuid;
    use zap_stream_core::ingress::ConnectionInfo;
    use zap_stream_db::{UserStream, UserStreamState};

    const TEST_STREAM_UUID: &str = "00000000-0000-0000-0000-000000000001";

    fn sample_stream(state: UserStreamState) -> UserStream {
        let mut stream = UserStream::default();
        stream.id = TEST_STREAM_UUID.to_string();
        stream.user_id = 1;
        stream.starts = Utc::now();
        stream.state = state;
        stream
    }

    fn test_connection_info() -> ConnectionInfo {
        ConnectionInfo {
            id: Uuid::parse_str(TEST_STREAM_UUID).unwrap(),
            endpoint: "test".to_string(),
            ip_addr: "127.0.0.1".to_string(),
            app_name: "test".to_string(),
            key: "test-key".to_string(),
        }
    }

    fn count_tag<'a>(tags: impl Iterator<Item = &'a Tag>, name: &str) -> usize {
        tags.filter(|tag| tag.as_slice().first().map(|v| v.as_str()) == Some(name))
            .count()
    }

    #[tokio::test]
    async fn stream_to_event_always_adds_viewer_count_from_stream_manager() {
        let keys = Keys::generate();
        let client = ClientBuilder::new().signer(keys).build();
        let publisher = N53Publisher::new(
            StreamManager::new("test-node".to_string()),
            client,
        );

        let stream = sample_stream(UserStreamState::Live);
        // No current_participants in extra_tags — stream_to_event should add it from stream_manager
        let event = publisher
            .stream_to_event(&stream, vec![], None)
            .await
            .unwrap();

        assert_eq!(count_tag(event.tags.iter(), "current_participants"), 1);
        let cp_value = event
            .tags
            .iter()
            .find(|t| t.as_slice().first().map(|v| v.as_str()) == Some("current_participants"))
            .and_then(|t| t.as_slice().get(1).map(|v| v.to_string()))
            .unwrap();
        assert_eq!(cp_value, "0");
    }

    #[tokio::test]
    async fn stream_to_event_uses_stream_manager_viewer_count() {
        let keys = Keys::generate();
        let client = ClientBuilder::new().signer(keys).build();
        let sm = StreamManager::new("test-node".to_string());

        // Register stream in stream manager, then set viewer count
        let conn = test_connection_info();
        sm.add_active_stream("deadbeef", 1, 30.0, "1920x1080", &conn, vec![], None)
            .await;
        sm.set_viewer_count(TEST_STREAM_UUID, 42).await;

        let publisher = N53Publisher::new(sm, client);
        let stream = sample_stream(UserStreamState::Live);

        let event = publisher
            .stream_to_event(&stream, vec![], None)
            .await
            .unwrap();

        let cp_tags: Vec<_> = event
            .tags
            .iter()
            .filter(|t| t.as_slice().first().map(|v| v.as_str()) == Some("current_participants"))
            .collect();
        assert_eq!(cp_tags.len(), 1);
        assert_eq!(cp_tags[0].as_slice()[1], "42");
    }

    #[tokio::test]
    async fn stream_to_event_live_stream_defaults_to_zero_viewers() {
        let keys = Keys::generate();
        let client = ClientBuilder::new().signer(keys).build();
        let publisher = N53Publisher::new(
            StreamManager::new("test-node".to_string()),
            client,
        );

        let stream = sample_stream(UserStreamState::Live);
        let event = publisher
            .stream_to_event(&stream, vec![], None)
            .await
            .unwrap();

        let cp_tags: Vec<_> = event
            .tags
            .iter()
            .filter(|t| t.as_slice().first().map(|v| v.as_str()) == Some("current_participants"))
            .collect();
        assert_eq!(cp_tags.len(), 1);
        assert_eq!(cp_tags[0].as_slice()[1], "0");
    }

    #[tokio::test]
    async fn stream_to_event_non_live_no_viewer_count() {
        let keys = Keys::generate();
        let client = ClientBuilder::new().signer(keys).build();
        let publisher = N53Publisher::new(
            StreamManager::new("test-node".to_string()),
            client,
        );

        let stream = sample_stream(UserStreamState::Ended);
        let event = publisher
            .stream_to_event(&stream, vec![], None)
            .await
            .unwrap();

        assert_eq!(count_tag(event.tags.iter(), "current_participants"), 0);
    }

    #[tokio::test]
    async fn stream_to_event_uses_provided_alt() {
        let keys = Keys::generate();
        let client = ClientBuilder::new().signer(keys).build();
        let publisher = N53Publisher::new(
            StreamManager::new("test-node".to_string()),
            client,
        );

        let stream = sample_stream(UserStreamState::Planned);
        let event = publisher
            .stream_to_event(&stream, vec![], Some("custom-alt".to_string()))
            .await
            .unwrap();

        assert_eq!(count_tag(event.tags.iter(), "alt"), 1);
        let alt_value = event
            .tags
            .iter()
            .find(|tag| tag.as_slice().first().map(|v| v.as_str()) == Some("alt"))
            .and_then(|tag| tag.as_slice().get(1).map(|v| v.to_string()))
            .unwrap();
        assert_eq!(alt_value, "custom-alt");
    }

    #[tokio::test]
    async fn stream_to_event_no_alt_when_none() {
        let keys = Keys::generate();
        let client = ClientBuilder::new().signer(keys).build();
        let publisher = N53Publisher::new(
            StreamManager::new("test-node".to_string()),
            client,
        );

        let stream = sample_stream(UserStreamState::Planned);
        let event = publisher.stream_to_event(&stream, vec![], None).await.unwrap();

        assert_eq!(count_tag(event.tags.iter(), "alt"), 0);
    }
}
