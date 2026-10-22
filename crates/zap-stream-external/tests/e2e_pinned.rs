mod common;

use common::api_client::ApiClient;
use common::config::TestConfig;
use common::db::TestDb;
use common::docker;
use common::ffmpeg::FfmpegStream;
use common::nostr_relay::{self, NostrRelay};
use nostr_sdk::{Client, Event, EventBuilder, Keys, Kind, Tag, Timestamp, ToBech32};
use std::time::Duration;
use uuid::Uuid;

/// Pin and unpin a live chat message on a live stream via PATCH /event and
/// verify the kind 30311 event on the relay carries (then drops) the NIP-53
/// `pinned` tag without disturbing other metadata.
#[tokio::test]
#[ignore]
async fn e2e_pin_live_chat_message() {
    let config = TestConfig::from_env();
    let total_steps = 8;

    // Unique token for this test run so we never match stale relay events
    let run_id = &Uuid::new_v4().to_string()[..8];
    println!("[INFO] Test run_id: {run_id}");

    // ── Step 1: Prerequisites ──────────────────────────────────────────
    println!("[TEST] Step 1/{total_steps}: Check prerequisites");
    assert!(
        docker::check_docker_available().await,
        "Docker is not running"
    );
    assert!(
        docker::command_exists("ffmpeg").await,
        "ffmpeg not found on PATH"
    );
    let ext_container = docker::detect_container("zap-stream-external")
        .await
        .or(config.external_container.clone())
        .expect("Cannot find zap-stream-external container");
    println!("[PASS] Step 1/{total_steps}: Check prerequisites");

    let test_keys = Keys::generate();
    let test_nsec = test_keys.secret_key().to_bech32().expect("bech32 nsec");
    let client = ApiClient::new(&test_nsec, &config.api_base_url()).await;
    let db = TestDb::connect(&config.db_connection_string()).await;
    db.ensure_user_exists(&client.pubkey_hex()).await;

    // ── Step 2: Set an account default title (PATCH without id) ────────
    println!("[TEST] Step 2/{total_steps}: Set account default title");
    let title = format!("Pin Show {run_id}");
    client
        .patch_event(&serde_json::json!({ "title": title }))
        .await;
    let account = client.get_account().await;
    let external_id = db
        .get_external_id(&client.pubkey_hex())
        .await
        .expect("No external_id after API call");
    let rtmps = account["endpoints"]
        .as_array()
        .expect("no endpoints")
        .iter()
        .find(|e| e["name"].as_str().unwrap_or("").starts_with("RTMPS-"))
        .expect("No RTMPS endpoint");
    let rtmp_url = rtmps["url"].as_str().unwrap();
    let rtmp_key = rtmps["key"].as_str().unwrap();
    println!("[PASS] Step 2/{total_steps}: Account default title set");

    // ── Step 3: Go live on the account key ─────────────────────────────
    println!("[TEST] Step 3/{total_steps}: Stream via RTMPS on the account key");
    let stream_start_ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut ffmpeg = FfmpegStream::start_rtmps(rtmp_url, rtmp_key, 150, 1000).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(ffmpeg.is_running(), "FFmpeg died immediately");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs = docker::get_docker_logs_since(&ext_container, &stream_start_ts).await;
    let connected_marker = format!("live_input.connected for input_id: {}", external_id);
    assert!(
        logs.contains(&connected_marker),
        "Missing webhook for this user's Live Input: '{}'",
        connected_marker
    );
    let stream_id = db
        .get_latest_stream_id(&client.pubkey_hex())
        .await
        .expect("No stream created for this user after webhook");
    println!("[PASS] Step 3/{total_steps}: Live (stream_id={stream_id})");

    let relay = NostrRelay::connect(&config.nostr_relay_url).await;
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - 600);

    // ── Step 4: LIVE event has the title and no pin ────────────────────
    println!("[TEST] Step 4/{total_steps}: LIVE event has title and no pin");
    let latest = latest_event(&relay, since, &stream_id).await;
    assert_eq!(
        nostr_relay::get_tag_value(&latest, "status").as_deref(),
        Some("live")
    );
    assert_eq!(
        nostr_relay::get_tag_value(&latest, "title").as_deref(),
        Some(title.as_str())
    );
    assert!(!nostr_relay::has_tag(&latest, "pinned"), "Unexpected pin");
    println!("[PASS] Step 4/{total_steps}: LIVE event has title and no pin");

    // Publish a real kind 1311 live chat message referencing the stream
    let a_tag = format!("30311:{}:{}", latest.pubkey.to_hex(), stream_id);
    let chat = publish_chat_message(&config.nostr_relay_url, &test_keys, &a_tag, run_id).await;
    let chat_id = chat.id.to_hex();

    // ── Step 5: Pin the message ────────────────────────────────────────
    println!("[TEST] Step 5/{total_steps}: Pin chat message {chat_id}");
    client
        .patch_event(&serde_json::json!({ "id": stream_id, "pinned": chat_id }))
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let latest = latest_event(&relay, since, &stream_id).await;
    assert_eq!(
        nostr_relay::get_all_tag_values(&latest, "pinned"),
        vec![chat_id.clone()],
        "Latest 30311 does not carry exactly the pinned id"
    );
    assert_eq!(
        nostr_relay::get_tag_value(&latest, "title").as_deref(),
        Some(title.as_str()),
        "Pin changed the title"
    );
    assert_eq!(
        nostr_relay::get_tag_value(&latest, "status").as_deref(),
        Some("live")
    );
    println!("[PASS] Step 5/{total_steps}: Pinned on the relay");

    // ── Step 6: Pin without id only touches account defaults ───────────
    println!("[TEST] Step 6/{total_steps}: PATCH without id leaves the live pin alone");
    let other_id = "f".repeat(64);
    client
        .patch_event(&serde_json::json!({ "pinned": other_id }))
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let latest = latest_event(&relay, since, &stream_id).await;
    assert_eq!(
        nostr_relay::get_all_tag_values(&latest, "pinned"),
        vec![chat_id.clone()],
        "PATCH without id changed the live stream's pin"
    );
    println!("[PASS] Step 6/{total_steps}: Live pin unchanged");

    // ── Step 7: Unpin ──────────────────────────────────────────────────
    println!("[TEST] Step 7/{total_steps}: Unpin with empty string");
    client
        .patch_event(&serde_json::json!({ "id": stream_id, "pinned": "" }))
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let latest = latest_event(&relay, since, &stream_id).await;
    assert!(
        !nostr_relay::has_tag(&latest, "pinned"),
        "Latest 30311 still carries a pinned tag after unpin"
    );
    assert_eq!(
        nostr_relay::get_tag_value(&latest, "title").as_deref(),
        Some(title.as_str()),
        "Unpin changed the title"
    );
    println!("[PASS] Step 7/{total_steps}: Unpinned on the relay");

    // ── Step 8: End stream ─────────────────────────────────────────────
    println!("[TEST] Step 8/{total_steps}: Stop stream");
    ffmpeg.stop().await;
    relay.disconnect().await;
    println!("[PASS] Step 8/{total_steps}: Stream stopped");
}

async fn latest_event(relay: &NostrRelay, since: Timestamp, stream_id: &str) -> Event {
    relay
        .query_30311_events(since, Some(stream_id))
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("No kind 30311 events for stream_id={stream_id}"))
}

async fn publish_chat_message(relay_url: &str, keys: &Keys, a_tag: &str, run_id: &str) -> Event {
    let client = Client::builder().signer(keys.clone()).build();
    client.add_relay(relay_url).await.expect("add relay failed");
    client.connect().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let eb = EventBuilder::new(Kind::LiveEventMessage, format!("Product drop {run_id}"))
        .tag(Tag::parse(["a", a_tag]).expect("valid a tag"));
    let event = client.sign_event_builder(eb).await.expect("signing failed");
    client.send_event(&event).await.expect("publish 1311 failed");
    client.disconnect().await;
    event
}
