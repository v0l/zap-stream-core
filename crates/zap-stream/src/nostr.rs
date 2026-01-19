use crate::stream_manager::StreamManager;
use anyhow::Result;
use nostr_sdk::prelude::Coordinate;
use nostr_sdk::{
    Client, Event, EventBuilder, JsonUtil, Kind, NostrSigner, Tag, Timestamp, ToBech32,
};
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
        self.client.send_event(ev).await?;
        Ok(())
    }

    pub async fn stream_to_event(
        &self,
        stream: &UserStream,
        extra_tags: Vec<Tag>,
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

        // Add current viewer count for live streams
        if stream.state == UserStreamState::Live {
            let viewer_count = self.stream_manager.get_viewer_count(&stream.id).await;
            tags.push(Tag::parse(&[
                "current_participants".to_string(),
                viewer_count.to_string(),
            ])?);
        }

        let pubkey = self.client.signer().await?.get_public_key().await?;
        let coord = Coordinate::new(Kind::LiveEvent, pubkey).identifier(&stream.id);
        tags.push(Tag::parse([
            "alt",
            &format!(
                "Watch live on https://zap.stream/{}",
                nostr_sdk::nips::nip19::Nip19Coordinate {
                    coordinate: coord,
                    relays: vec![]
                }
                .to_bech32()?
            ),
        ])?);

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
