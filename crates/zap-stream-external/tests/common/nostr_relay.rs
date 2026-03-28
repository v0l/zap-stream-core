use nostr_sdk::{Client, Event, Filter, Keys, Kind, Timestamp};
use std::time::Duration;

pub struct NostrRelay {
    client: Client,
}

impl NostrRelay {
    /// Connect to a relay with ephemeral keys for NIP-42 authentication.
    pub async fn connect(relay_url: &str) -> Self {
        let keys = Keys::generate();
        let client = Client::builder().signer(keys).build();
        client.add_relay(relay_url).await.expect("add relay failed");
        client.connect().await;
        // Give relay a moment to complete NIP-42 handshake
        tokio::time::sleep(Duration::from_secs(2)).await;
        Self { client }
    }

    /// Query all kind 30311 events since `since`.
    /// Optionally filter by `d` tag value (stream_id).
    pub async fn query_30311_events(&self, since: Timestamp, d_tag: Option<&str>) -> Vec<Event> {
        let filter = Filter::new().kind(Kind::Custom(30311)).since(since);

        let timeout = Duration::from_secs(15);
        let mut events: Vec<Event> = self
            .client
            .fetch_events(filter, timeout)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

        if let Some(d) = d_tag {
            events.retain(|e| {
                e.tags.iter().any(|t| {
                    let s = t.as_slice();
                    s.len() >= 2 && s[0] == "d" && s[1] == d
                })
            });
        }

        // Most recent first
        events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        events
    }

    /// Find the most recent event matching a pubkey (via p-tag host) and status.
    pub fn find_user_event<'a>(
        events: &'a [Event],
        pubkey_hex: &str,
        status: &str,
    ) -> Option<&'a Event> {
        events.iter().find(|e| {
            let has_p = e.tags.iter().any(|t| {
                let s = t.as_slice();
                s.len() >= 2 && s[0] == "p" && s[1] == pubkey_hex
            });
            let has_status = e.tags.iter().any(|t| {
                let s = t.as_slice();
                s.len() >= 2 && s[0] == "status" && s[1] == status
            });
            has_p && has_status
        })
    }

    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }
}

/// Extract the first value for a given tag name from a Nostr event.
pub fn get_tag_value(event: &Event, tag_name: &str) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        if s.len() >= 2 && s[0] == tag_name {
            Some(s[1].to_string())
        } else {
            None
        }
    })
}

/// Check whether a tag with the given name exists in the event.
pub fn has_tag(event: &Event, tag_name: &str) -> bool {
    event
        .tags
        .iter()
        .any(|t| t.as_slice().first().map(|v| v.as_str()) == Some(tag_name))
}

/// Collect all values for a given tag name (e.g. multiple "t" tags).
pub fn get_all_tag_values(event: &Event, tag_name: &str) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|t| {
            let s = t.as_slice();
            if s.len() >= 2 && s[0] == tag_name {
                Some(s[1].to_string())
            } else {
                None
            }
        })
        .collect()
}
