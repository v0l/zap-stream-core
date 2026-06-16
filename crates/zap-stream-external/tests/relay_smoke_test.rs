use nostr_sdk::{Client, EventBuilder, Filter, Keys, Kind, Timestamp};
use std::time::Duration;

/// Prove we can: 1) auth to relay, 2) write an event, 3) read it back.
#[tokio::test]
#[ignore]
async fn relay_write_read_roundtrip() {
    let relay_url = std::env::var("NOSTR_RELAY_URL")
        .unwrap_or_else(|_| "ws://localhost:3334".to_string());

    // Generate a keypair and connect
    let keys = Keys::generate();
    let client = Client::builder().signer(keys.clone()).build();
    client.add_relay(&relay_url).await.expect("add relay failed");
    client.connect().await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Write: publish a kind 1 text note
    let marker = format!("relay-smoke-test-{}", chrono::Utc::now().timestamp());
    let event = client
        .sign_event_builder(EventBuilder::text_note(&marker))
        .await
        .expect("failed to sign event");
    let event_id = event.id;

    let send_result = client.send_event(&event).await;
    println!("Send result: {:?}", send_result);
    assert!(
        send_result.is_ok(),
        "Failed to send event to relay: {:?}",
        send_result.err()
    );

    // Read: fetch it back by ID
    tokio::time::sleep(Duration::from_secs(1)).await;
    let filter = Filter::new()
        .id(event_id)
        .kind(Kind::TextNote)
        .since(Timestamp::from(chrono::Utc::now().timestamp() as u64 - 60));
    let fetched: Vec<_> = client
        .fetch_events(filter, Duration::from_secs(5))
        .await
        .expect("fetch failed")
        .into_iter()
        .collect();

    println!("Fetched {} events", fetched.len());
    assert_eq!(fetched.len(), 1, "Expected exactly 1 event back");
    assert_eq!(fetched[0].id, event_id);
    assert_eq!(fetched[0].content, marker);
    println!("PASS: wrote event {} and read it back", event_id);

    client.disconnect().await;
}
