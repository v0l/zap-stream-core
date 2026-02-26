mod common;

use common::api_client::ApiClient;
use common::config::TestConfig;
use common::db::TestDb;
use common::docker;
use common::ffmpeg::FfmpegStream;
use common::nostr_relay::{self, NostrRelay};
use nostr_sdk::Timestamp;
use std::time::Duration;

/// Safe test-only nsec (not used for anything real).
const TEST_NSEC: &str = "nsec107gexedhvf97ej83jzalley9wt682mlgy9ty5xwsp98vnph09fysssnzlk";

#[tokio::test]
#[ignore]
async fn e2e_single_user_lifecycle() {
    let config = TestConfig::from_env();
    let total_steps = 16;

    // ── Step 1/16: Prerequisites ──────────────────────────────────────
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

    // ── Step 2/16: Initial database state ─────────────────────────────
    println!("[TEST] Step 2/{total_steps}: Check initial database state");
    let client = ApiClient::new(TEST_NSEC, &config.api_base_url()).await;
    let db = TestDb::connect(&config.db_connection_string()).await;
    db.ensure_user_exists(&client.pubkey_hex()).await;
    let ext_id_before = db.get_external_id(&client.pubkey_hex()).await;
    println!(
        "[PASS] Step 2/{total_steps}: Initial DB state (external_id={:?})",
        ext_id_before
    );

    // ── Step 3/16: API call with NIP-98 auth ──────────────────────────
    println!("[TEST] Step 3/{total_steps}: API call creates/reuses Live Input");
    let account = client.get_account().await;
    assert!(
        account.get("endpoints").is_some(),
        "No 'endpoints' in account response"
    );
    println!("[PASS] Step 3/{total_steps}: API call returned endpoints");

    // ── Step 4/16: DB contains valid external_id ──────────────────────
    println!("[TEST] Step 4/{total_steps}: Database contains valid external_id");
    let ext_id_after = db
        .get_external_id(&client.pubkey_hex())
        .await
        .expect("No external_id after API call");
    assert!(
        ext_id_after.len() == 32 && ext_id_after.chars().all(|c| c.is_ascii_hexdigit()),
        "Invalid external_id format: {}",
        ext_id_after
    );
    println!(
        "[PASS] Step 4/{total_steps}: external_id = {}",
        ext_id_after
    );

    // ── Step 5/16: RTMPS endpoint validation ──────────────────────────
    println!("[TEST] Step 5/{total_steps}: RTMPS endpoint validation");
    let endpoints = account["endpoints"]
        .as_array()
        .expect("endpoints is not an array");
    let rtmps = endpoints
        .iter()
        .find(|e| {
            e["name"]
                .as_str()
                .unwrap_or("")
                .starts_with("RTMPS-")
        })
        .expect("No RTMPS endpoint found");
    let rtmp_url = rtmps["url"].as_str().expect("No RTMPS url");
    let rtmp_key = rtmps["key"].as_str().expect("No RTMPS key");
    assert!(
        rtmp_url.starts_with("rtmps://"),
        "RTMPS URL doesn't start with rtmps://: {}",
        rtmp_url
    );
    assert!(!rtmp_key.is_empty(), "RTMPS key is empty");
    println!(
        "[PASS] Step 5/{total_steps}: RTMPS endpoint url={} key={}...",
        rtmp_url,
        &rtmp_key[..20.min(rtmp_key.len())]
    );

    // ── Step 6/16: SRT endpoint validation ────────────────────────────
    println!("[TEST] Step 6/{total_steps}: SRT endpoint validation");
    let srt = endpoints.iter().find(|e| {
        e["name"]
            .as_str()
            .unwrap_or("")
            .starts_with("SRT-")
    });
    if let Some(srt_ep) = srt {
        let srt_url = srt_ep["url"].as_str().unwrap_or("");
        let srt_key = srt_ep["key"].as_str().unwrap_or("");
        assert!(
            srt_url.starts_with("srt://"),
            "SRT URL doesn't start with srt://: {}",
            srt_url
        );
        assert!(
            srt_key.contains("streamid=") && srt_key.contains("&passphrase="),
            "SRT key missing streamid or passphrase: {}",
            srt_key
        );
        println!("[PASS] Step 6/{total_steps}: SRT endpoint validated");
    } else {
        println!("[PASS] Step 6/{total_steps}: SRT endpoint not available (skipped)");
    }

    // ── Step 7/16: Idempotency ────────────────────────────────────────
    println!("[TEST] Step 7/{total_steps}: Second API call reuses same UID");
    let account2 = client.get_account().await;
    let rtmps_2 = account2["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["name"]
                .as_str()
                .unwrap_or("")
                .starts_with("RTMPS-")
        })
        .expect("No RTMPS endpoint on second call");
    let rtmp_key_2 = rtmps_2["key"].as_str().unwrap();
    let ext_id_final = db
        .get_external_id(&client.pubkey_hex())
        .await
        .expect("No external_id after second call");
    assert_eq!(
        ext_id_after, ext_id_final,
        "external_id changed between calls"
    );
    assert_eq!(rtmp_key, rtmp_key_2, "Stream key changed between calls");
    println!("[PASS] Step 7/{total_steps}: Idempotency verified");

    // ── Step 8/16: Custom keys ────────────────────────────────────────
    println!("[TEST] Step 8/{total_steps}: Custom keys - create and list");
    let create_resp = client
        .create_key(
            "E2E Test Stream",
            "External backend custom key test",
            &["test", "e2e"],
        )
        .await;
    let custom_key = create_resp["key"]
        .as_str()
        .expect("No 'key' in create response");
    assert!(!custom_key.is_empty(), "Custom key is empty");

    let keys_list = client.list_keys().await;
    let keys_arr = keys_list.as_array().expect("keys list is not an array");
    assert!(!keys_arr.is_empty(), "Keys list is empty");
    let key_entry = keys_arr
        .iter()
        .find(|k| k["key"].as_str() == Some(custom_key))
        .expect("Created key not found in list");
    let custom_key_stream_id = key_entry["stream_id"]
        .as_str()
        .expect("No stream_id on key entry")
        .to_string();

    let ck_ext_id = db.get_custom_key_external_id(&custom_key_stream_id).await;
    println!(
        "[PASS] Step 8/{total_steps}: Custom key created (stream_id={}, ck_ext_id={:?})",
        custom_key_stream_id, ck_ext_id
    );

    // ── Step 9/16: Stream via RTMPS ───────────────────────────────────
    println!("[TEST] Step 9/{total_steps}: Stream via RTMPS to Cloudflare");
    let mut ffmpeg = FfmpegStream::start_rtmps(rtmp_url, rtmp_key, 30, 1000).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(ffmpeg.is_running(), "FFmpeg died immediately");
    println!("[PASS] Step 9/{total_steps}: RTMPS stream started");

    // ── Step 10/16: Webhook START ─────────────────────────────────────
    println!("[TEST] Step 10/{total_steps}: Webhooks trigger stream START");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs = docker::get_docker_logs(&ext_container, 200).await;
    assert!(
        logs.contains("live_input.connected"),
        "Missing live_input.connected webhook in logs"
    );
    assert!(
        logs.contains("Published stream event"),
        "Missing 'Published stream event' in logs"
    );
    println!("[PASS] Step 10/{total_steps}: Webhook START received");

    // ── Step 11/16: Verify LIVE Nostr event ───────────────────────────
    println!("[TEST] Step 11/{total_steps}: Verify LIVE Nostr event (kind 30311)");
    let relay = NostrRelay::connect(&config.nostr_relay_url).await;
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - 600);
    let events = relay.query_30311_events(since, None).await;
    assert!(!events.is_empty(), "No kind 30311 events found");

    let live_event = events
        .iter()
        .find(|e| nostr_relay::get_tag_value(e, "status").as_deref() == Some("live"))
        .expect("No LIVE kind 30311 event found");

    assert!(
        nostr_relay::has_tag(live_event, "streaming"),
        "LIVE event missing 'streaming' tag"
    );
    assert!(
        nostr_relay::has_tag(live_event, "starts"),
        "LIVE event missing 'starts' tag"
    );
    assert!(
        !nostr_relay::has_tag(live_event, "ends"),
        "LIVE event should not have 'ends' tag"
    );
    println!("[PASS] Step 11/{total_steps}: LIVE Nostr event verified");

    // ── Step 12/16: End stream ────────────────────────────────────────
    println!("[TEST] Step 12/{total_steps}: End stream and verify END webhooks");
    ffmpeg.stop().await;
    tokio::time::sleep(Duration::from_secs(15)).await;
    let logs = docker::get_docker_logs(&ext_container, 200).await;
    assert!(
        logs.contains("live_input.disconnected"),
        "Missing live_input.disconnected webhook in logs"
    );
    assert!(
        logs.contains("Stream ended"),
        "Missing 'Stream ended' in logs"
    );
    println!("[PASS] Step 12/{total_steps}: Stream END webhooks received");

    // ── Step 13/16: Verify ENDED Nostr event ──────────────────────────
    println!("[TEST] Step 13/{total_steps}: Verify ENDED Nostr event");
    let events = relay.query_30311_events(since, None).await;
    let ended_event = events
        .iter()
        .find(|e| nostr_relay::get_tag_value(e, "status").as_deref() == Some("ended"))
        .expect("No ENDED kind 30311 event found");

    assert!(
        nostr_relay::has_tag(ended_event, "ends"),
        "ENDED event missing 'ends' tag"
    );
    let streaming_val = nostr_relay::get_tag_value(ended_event, "streaming");
    assert!(
        streaming_val.is_none() || streaming_val.as_deref() == Some(""),
        "ENDED event should not have 'streaming' tag (got {:?})",
        streaming_val
    );
    println!("[PASS] Step 13/{total_steps}: ENDED Nostr event verified");

    // ── Steps 14-16: Custom key stream lifecycle ──────────────────────

    // ── Step 14/16: Stream with custom key ────────────────────────────
    println!("[TEST] Step 14/{total_steps}: Stream with custom key");
    let mut ck_ffmpeg = FfmpegStream::start_rtmps(rtmp_url, custom_key, 30, 800).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(ck_ffmpeg.is_running(), "Custom key FFmpeg died immediately");

    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs = docker::get_docker_logs(&ext_container, 200).await;
    assert!(
        logs.contains("live_input.connected"),
        "Missing live_input.connected webhook for custom key"
    );

    if let Some(state) = db.get_stream_state(&custom_key_stream_id).await {
        assert!(
            state == 2 || state == 3,
            "Custom key stream state should be Live(2) or Ended(3), got {}",
            state
        );
    }
    println!("[PASS] Step 14/{total_steps}: Custom key stream started");

    // ── Step 15/16: Custom key Nostr event metadata ───────────────────
    println!("[TEST] Step 15/{total_steps}: Custom key Nostr event metadata");
    let ck_events = relay
        .query_30311_events(since, Some(&custom_key_stream_id))
        .await;
    assert!(
        !ck_events.is_empty(),
        "No kind 30311 events for custom key stream_id={}",
        custom_key_stream_id
    );
    let ck_event = &ck_events[0];
    let ck_status = nostr_relay::get_tag_value(ck_event, "status");
    assert!(
        ck_status.as_deref() == Some("live") || ck_status.as_deref() == Some("ended"),
        "Custom key event status should be live or ended, got {:?}",
        ck_status
    );

    let title = nostr_relay::get_tag_value(ck_event, "title");
    assert_eq!(
        title.as_deref(),
        Some("E2E Test Stream"),
        "Custom key event title mismatch: {:?}",
        title
    );
    let summary = nostr_relay::get_tag_value(ck_event, "summary");
    assert_eq!(
        summary.as_deref(),
        Some("External backend custom key test"),
        "Custom key event summary mismatch: {:?}",
        summary
    );
    let t_tags = nostr_relay::get_all_tag_values(ck_event, "t");
    assert!(
        t_tags.contains(&"test".to_string()),
        "Custom key event missing 'test' t-tag (got {:?})",
        t_tags
    );
    assert!(
        t_tags.contains(&"e2e".to_string()),
        "Custom key event missing 'e2e' t-tag (got {:?})",
        t_tags
    );
    println!("[PASS] Step 15/{total_steps}: Custom key Nostr metadata verified");

    // ── Step 16/16: Custom key ENDED Nostr event ──────────────────────
    println!("[TEST] Step 16/{total_steps}: Custom key stream ENDED Nostr event");
    ck_ffmpeg.stop().await;
    tokio::time::sleep(Duration::from_secs(15)).await;

    let ck_events = relay
        .query_30311_events(since, Some(&custom_key_stream_id))
        .await;
    let ck_ended = ck_events
        .iter()
        .find(|e| nostr_relay::get_tag_value(e, "status").as_deref() == Some("ended"))
        .expect("No ENDED event for custom key stream");

    assert!(
        nostr_relay::has_tag(ck_ended, "ends"),
        "Custom key ENDED event missing 'ends' tag"
    );
    let ends_val = nostr_relay::get_tag_value(ck_ended, "ends").unwrap();
    assert!(
        !ends_val.is_empty(),
        "Custom key ENDED event 'ends' tag is empty"
    );
    println!("[PASS] Step 16/{total_steps}: Custom key ENDED event verified");

    relay.disconnect().await;
    println!("\n====== ALL {total_steps}/{total_steps} STEPS PASSED ======");
}
