mod common;

use common::api_client::ApiClient;
use common::config::TestConfig;
use common::db::TestDb;
use common::docker;
use common::ffmpeg::FfmpegStream;
use common::nostr_relay::{self, NostrRelay};
use nostr_sdk::{Keys, ToBech32, Timestamp};
use std::time::Duration;

#[tokio::test]
#[ignore]
async fn e2e_multi_user_concurrent_streaming() {
    let config = TestConfig::from_env();
    let total_steps = 14;

    // ── Step 1/14: Prerequisites ──────────────────────────────────────
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
    let _db_container = docker::detect_container("db-1")
        .await
        .or(config.db_container.clone())
        .expect("Cannot find db container");
    println!("[PASS] Step 1/{total_steps}: Check prerequisites");

    // ── Step 2/14: Database setup ─────────────────────────────────────
    println!("[TEST] Step 2/{total_steps}: Database setup for both users");
    let nsec_a = Keys::generate().secret_key().to_bech32().expect("bech32 nsec");
    let nsec_b = Keys::generate().secret_key().to_bech32().expect("bech32 nsec");
    let client_a = ApiClient::new(&nsec_a, &config.api_base_url()).await;
    let client_b = ApiClient::new(&nsec_b, &config.api_base_url()).await;
    let db = TestDb::connect(&config.db_connection_string()).await;
    db.ensure_user_exists(&client_a.pubkey_hex()).await;
    db.ensure_user_exists(&client_b.pubkey_hex()).await;
    println!(
        "[PASS] Step 2/{total_steps}: Users A={} B={} ensured in DB",
        &client_a.pubkey_hex()[..12],
        &client_b.pubkey_hex()[..12]
    );

    // ── Step 3/14: API credentials for both users ─────────────────────
    println!("[TEST] Step 3/{total_steps}: Get stream credentials for both users");
    let account_a = client_a.get_account().await;
    let account_b = client_b.get_account().await;
    assert!(
        account_a.get("endpoints").is_some(),
        "User A: no endpoints"
    );
    assert!(
        account_b.get("endpoints").is_some(),
        "User B: no endpoints"
    );

    let endpoints_a = account_a["endpoints"].as_array().unwrap();
    let endpoints_b = account_b["endpoints"].as_array().unwrap();
    let rtmps_a = endpoints_a
        .iter()
        .find(|e| e["name"].as_str().unwrap_or("").starts_with("RTMPS-"))
        .expect("User A: no RTMPS endpoint");
    let rtmps_b = endpoints_b
        .iter()
        .find(|e| e["name"].as_str().unwrap_or("").starts_with("RTMPS-"))
        .expect("User B: no RTMPS endpoint");

    let rtmp_url_a = rtmps_a["url"].as_str().unwrap();
    let rtmp_key_a = rtmps_a["key"].as_str().unwrap();
    let rtmp_url_b = rtmps_b["url"].as_str().unwrap();
    let rtmp_key_b = rtmps_b["key"].as_str().unwrap();
    println!("[PASS] Step 3/{total_steps}: Both users have stream credentials");

    // ── Step 4/14: Unique external_ids ────────────────────────────────
    println!("[TEST] Step 4/{total_steps}: Unique external_ids per user");
    let ext_id_a = db
        .get_external_id(&client_a.pubkey_hex())
        .await
        .expect("User A: no external_id");
    let ext_id_b = db
        .get_external_id(&client_b.pubkey_hex())
        .await
        .expect("User B: no external_id");
    assert_ne!(
        ext_id_a, ext_id_b,
        "Users A and B have the same external_id"
    );
    assert!(
        ext_id_a.len() == 32 && ext_id_a.chars().all(|c| c.is_ascii_hexdigit()),
        "User A external_id invalid: {}",
        ext_id_a
    );
    assert!(
        ext_id_b.len() == 32 && ext_id_b.chars().all(|c| c.is_ascii_hexdigit()),
        "User B external_id invalid: {}",
        ext_id_b
    );
    println!(
        "[PASS] Step 4/{total_steps}: Unique IDs: A={} B={}",
        ext_id_a, ext_id_b
    );

    // ── Step 5/14: User A starts streaming ────────────────────────────
    println!("[TEST] Step 5/{total_steps}: User A starts streaming");
    let a_start_ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut ffmpeg_a = FfmpegStream::start_rtmps(rtmp_url_a, rtmp_key_a, 120, 1000).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(ffmpeg_a.is_running(), "User A FFmpeg died immediately");
    println!("[PASS] Step 5/{total_steps}: User A streaming");

    // ── Step 6/14: User A webhook START ───────────────────────────────
    println!("[TEST] Step 6/{total_steps}: User A webhook START");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs = docker::get_docker_logs_since(&ext_container, &a_start_ts).await;
    let a_connected = format!("live_input.connected for input_id: {}", ext_id_a);
    assert!(
        logs.contains(&a_connected),
        "Missing User A connected webhook.\n\
         Expected: '{}'\n\
         in logs since {} ({} bytes)",
        a_connected,
        a_start_ts,
        logs.len(),
    );
    assert!(
        logs.contains("Published stream event"),
        "Missing 'Published stream event' for User A"
    );
    // Isolation: User B should NOT have connected yet
    let b_connected = format!("live_input.connected for input_id: {}", ext_id_b);
    assert!(
        !logs.contains(&b_connected),
        "User B connected webhook appeared before User B started streaming"
    );
    println!("[PASS] Step 6/{total_steps}: User A webhook START (isolated)");

    // ── Step 7/14: User B starts streaming ────────────────────────────
    println!("[TEST] Step 7/{total_steps}: User B starts streaming (concurrent)");
    let b_start_ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut ffmpeg_b = FfmpegStream::start_rtmps(rtmp_url_b, rtmp_key_b, 120, 800).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(ffmpeg_a.is_running(), "User A FFmpeg died while B started");
    assert!(ffmpeg_b.is_running(), "User B FFmpeg died immediately");
    println!("[PASS] Step 7/{total_steps}: Both users streaming concurrently");

    // ── Step 8/14: User B webhook START ───────────────────────────────
    println!("[TEST] Step 8/{total_steps}: User B webhook START");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs = docker::get_docker_logs_since(&ext_container, &b_start_ts).await;
    let b_connected = format!("live_input.connected for input_id: {}", ext_id_b);
    assert!(
        logs.contains(&b_connected),
        "Missing User B connected webhook.\n\
         Expected: '{}'\n\
         in logs since {} ({} bytes)",
        b_connected,
        b_start_ts,
        logs.len(),
    );
    println!("[PASS] Step 8/{total_steps}: User B webhook START received");

    // ── Step 9/14: Per-user LIVE Nostr events ─────────────────────────
    println!("[TEST] Step 9/{total_steps}: Verify per-user LIVE Nostr events");
    let relay = NostrRelay::connect(&config.nostr_relay_url).await;
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - 600);
    let events = relay.query_30311_events(since, None).await;

    let event_a = NostrRelay::find_user_event(&events, &client_a.pubkey_hex(), "live")
        .expect("No LIVE event for User A");
    let event_b = NostrRelay::find_user_event(&events, &client_b.pubkey_hex(), "live")
        .expect("No LIVE event for User B");

    let d_tag_a = nostr_relay::get_tag_value(event_a, "d").expect("User A event missing d-tag");
    let d_tag_b = nostr_relay::get_tag_value(event_b, "d").expect("User B event missing d-tag");
    assert_ne!(
        d_tag_a, d_tag_b,
        "User A and B have the same d-tag (stream_id)"
    );
    println!(
        "[PASS] Step 9/{total_steps}: Per-user LIVE events (A d={}, B d={})",
        d_tag_a, d_tag_b
    );

    // ── Step 10/14: Stream isolation — stop User A ────────────────────
    println!("[TEST] Step 10/{total_steps}: Stream isolation - stop User A");
    let a_stop_ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    ffmpeg_a.stop().await;
    assert!(!ffmpeg_a.is_running(), "User A FFmpeg still running after stop");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        ffmpeg_b.is_running(),
        "User B FFmpeg died when User A stopped"
    );
    println!("[PASS] Step 10/{total_steps}: User B still streaming after A stopped");

    // ── Step 11/14: User A disconnect webhook ─────────────────────────
    println!("[TEST] Step 11/{total_steps}: User A disconnect webhook (isolation)");
    tokio::time::sleep(Duration::from_secs(15)).await;
    let logs = docker::get_docker_logs_since(&ext_container, &a_stop_ts).await;
    let a_disconnected = format!("live_input.disconnected for input_id: {}", ext_id_a);
    let a_stream_ended = logs.contains("Stream ended");
    assert!(
        logs.contains(&a_disconnected) || a_stream_ended,
        "Missing User A disconnect.\n\
         Expected: '{}'\n\
         in logs since {} ({} bytes)",
        a_disconnected,
        a_stop_ts,
        logs.len(),
    );
    let b_disconnected = format!("live_input.disconnected for input_id: {}", ext_id_b);
    assert!(
        !logs.contains(&b_disconnected),
        "User B disconnected webhook appeared (isolation failure)"
    );
    assert!(
        ffmpeg_b.is_running(),
        "User B FFmpeg died after User A disconnect"
    );

    // Verify Nostr: User A ended, User B still live
    let events = relay.query_30311_events(since, None).await;
    let a_ended = NostrRelay::find_user_event(&events, &client_a.pubkey_hex(), "ended");
    assert!(a_ended.is_some(), "User A should have ENDED event");
    let b_still_live = NostrRelay::find_user_event(&events, &client_b.pubkey_hex(), "live");
    assert!(b_still_live.is_some(), "User B should still be LIVE");
    println!("[PASS] Step 11/{total_steps}: User A ended, User B still live (isolated)");

    // ── Step 12/14: Stop User B ───────────────────────────────────────
    println!("[TEST] Step 12/{total_steps}: Stop User B");
    let b_stop_ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    ffmpeg_b.stop().await;
    assert!(!ffmpeg_b.is_running(), "User B FFmpeg still running after stop");
    tokio::time::sleep(Duration::from_secs(15)).await;
    let logs = docker::get_docker_logs_since(&ext_container, &b_stop_ts).await;
    let b_disconnected = format!("live_input.disconnected for input_id: {}", ext_id_b);
    let b_stream_ended = logs.contains("Stream ended");
    assert!(
        logs.contains(&b_disconnected) || b_stream_ended,
        "Missing User B disconnect.\n\
         Expected: '{}'\n\
         in logs since {} ({} bytes)",
        b_disconnected,
        b_stop_ts,
        logs.len(),
    );
    println!("[PASS] Step 12/{total_steps}: User B stopped");

    // ── Step 13/14: Per-user ENDED Nostr events ───────────────────────
    println!("[TEST] Step 13/{total_steps}: Verify per-user ENDED Nostr events");
    let events = relay.query_30311_events(since, None).await;

    let ended_a = NostrRelay::find_user_event(&events, &client_a.pubkey_hex(), "ended")
        .expect("No ENDED event for User A");
    let ended_b = NostrRelay::find_user_event(&events, &client_b.pubkey_hex(), "ended")
        .expect("No ENDED event for User B");

    assert!(
        nostr_relay::has_tag(ended_a, "ends"),
        "User A ENDED event missing 'ends' tag"
    );
    assert!(
        nostr_relay::has_tag(ended_b, "ends"),
        "User B ENDED event missing 'ends' tag"
    );

    let d_tag_a_final =
        nostr_relay::get_tag_value(ended_a, "d").expect("User A ended event missing d-tag");
    let d_tag_b_final =
        nostr_relay::get_tag_value(ended_b, "d").expect("User B ended event missing d-tag");
    assert_ne!(d_tag_a_final, d_tag_b_final, "ENDED events have same d-tag");
    println!("[PASS] Step 13/{total_steps}: Per-user ENDED events verified");

    // ── Step 14/14: UID persistence ───────────────────────────────────
    println!("[TEST] Step 14/{total_steps}: UID persistence validation");
    let ext_id_a_final = db
        .get_external_id(&client_a.pubkey_hex())
        .await
        .expect("User A: external_id gone");
    let ext_id_b_final = db
        .get_external_id(&client_b.pubkey_hex())
        .await
        .expect("User B: external_id gone");
    assert_eq!(ext_id_a, ext_id_a_final, "User A external_id changed");
    assert_eq!(ext_id_b, ext_id_b_final, "User B external_id changed");
    println!("[PASS] Step 14/{total_steps}: UIDs persisted");

    relay.disconnect().await;
    println!("\n====== ALL {total_steps}/{total_steps} STEPS PASSED ======");
}
