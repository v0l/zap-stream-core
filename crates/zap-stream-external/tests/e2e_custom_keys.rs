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
async fn e2e_custom_key_management() {
    let config = TestConfig::from_env();
    let total_steps = 11;

    // ── Step 1/11: Prerequisites ──────────────────────────────────────
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

    let client = ApiClient::new(TEST_NSEC, &config.api_base_url()).await;
    let db = TestDb::connect(&config.db_connection_string()).await;
    db.ensure_user_exists(&client.pubkey_hex()).await;

    // Get RTMPS base URL for streaming later
    let account = client.get_account().await;
    let endpoints = account["endpoints"].as_array().expect("no endpoints");
    let rtmps = endpoints
        .iter()
        .find(|e| e["name"].as_str().unwrap_or("").starts_with("RTMPS-"))
        .expect("No RTMPS endpoint");
    let rtmp_url = rtmps["url"].as_str().unwrap();

    // ── Step 2/11: Create first custom key ────────────────────────────
    println!("[TEST] Step 2/{total_steps}: Create first custom key with metadata");
    let key1_resp = client
        .create_key(
            "Custom Key Test Stream",
            "E2E test of custom keys on external backend",
            &["test", "custom-key", "e2e"],
        )
        .await;
    let key1 = key1_resp["key"]
        .as_str()
        .expect("No 'key' in response")
        .to_string();
    assert!(!key1.is_empty(), "Key 1 is empty");
    println!(
        "[PASS] Step 2/{total_steps}: Key 1 created: {}...",
        &key1[..20.min(key1.len())]
    );

    // ── Step 3/11: Create second custom key ───────────────────────────
    println!("[TEST] Step 3/{total_steps}: Create second custom key (verify uniqueness)");
    let key2_resp = client
        .create_key("Second Custom Stream", "Testing multiple keys per user", &[])
        .await;
    let key2 = key2_resp["key"]
        .as_str()
        .expect("No 'key' in response")
        .to_string();
    assert!(!key2.is_empty(), "Key 2 is empty");
    assert_ne!(key1, key2, "Key 1 and Key 2 are the same");
    println!(
        "[PASS] Step 3/{total_steps}: Key 2 created: {}... (unique)",
        &key2[..20.min(key2.len())]
    );

    // ── Step 4/11: List keys ──────────────────────────────────────────
    println!("[TEST] Step 4/{total_steps}: List all keys");
    let keys_list = client.list_keys().await;
    let keys_arr = keys_list.as_array().expect("keys list is not an array");
    assert!(
        keys_arr.len() >= 2,
        "Expected at least 2 keys, got {}",
        keys_arr.len()
    );

    let entry1 = keys_arr
        .iter()
        .find(|k| k["key"].as_str() == Some(&key1))
        .expect("Key 1 not found in list");
    let entry2 = keys_arr
        .iter()
        .find(|k| k["key"].as_str() == Some(&key2))
        .expect("Key 2 not found in list");

    let stream_id_1 = entry1["stream_id"]
        .as_str()
        .expect("Key 1 missing stream_id")
        .to_string();
    let stream_id_2 = entry2["stream_id"]
        .as_str()
        .expect("Key 2 missing stream_id")
        .to_string();
    assert_ne!(
        stream_id_1, stream_id_2,
        "Key 1 and Key 2 have the same stream_id"
    );
    println!(
        "[PASS] Step 4/{total_steps}: {} keys listed, stream_ids unique ({}..., {}...)",
        keys_arr.len(),
        &stream_id_1[..8.min(stream_id_1.len())],
        &stream_id_2[..8.min(stream_id_2.len())]
    );

    // ── Step 5/11: Cloudflare API direct validation ───────────────────
    println!("[TEST] Step 5/{total_steps}: Cloudflare API direct validation");
    if let (Some(cf_token), Some(cf_account)) =
        (&config.cloudflare_api_token, &config.cloudflare_account_id)
    {
        let ck_ext_id = db
            .get_custom_key_external_id(&stream_id_1)
            .await
            .expect("No external_id for custom key 1");

        let cf_url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/stream/live_inputs/{}",
            cf_account, ck_ext_id
        );
        let http = reqwest::Client::new();
        let cf_resp: serde_json::Value = http
            .get(&cf_url)
            .header("Authorization", format!("Bearer {}", cf_token))
            .send()
            .await
            .expect("CF API call failed")
            .json()
            .await
            .expect("CF API invalid JSON");

        assert_eq!(
            cf_resp["success"].as_bool(),
            Some(true),
            "Cloudflare API returned success=false"
        );
        println!("[PASS] Step 5/{total_steps}: Cloudflare API validated");
    } else {
        println!(
            "[PASS] Step 5/{total_steps}: Cloudflare API validation skipped (no credentials)"
        );
    }

    // ── Step 6/11: Stream using custom key 1 ──────────────────────────
    println!("[TEST] Step 6/{total_steps}: Stream using custom key 1");

    // Get the custom key's Cloudflare external_id so we can assert on it specifically
    let ck1_external_id = db
        .get_custom_key_external_id(&stream_id_1)
        .await
        .expect("No external_id in DB for custom key 1");
    println!("[INFO] Custom key 1 external_id (CF Live Input): {}", ck1_external_id);

    // Capture log baseline before streaming so we only check NEW log entries
    let logs_before = docker::get_docker_logs(&ext_container, 200).await;
    let baseline_len = logs_before.len();

    let mut ffmpeg = FfmpegStream::start_rtmps(rtmp_url, &key1, 90, 1000).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(ffmpeg.is_running(), "FFmpeg died immediately");
    println!("[PASS] Step 6/{total_steps}: Custom key stream started");

    // ── Step 7/11: Webhook START for custom key ───────────────────────
    println!("[TEST] Step 7/{total_steps}: Webhook START for custom key (input_id={})", ck1_external_id);
    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs_after = docker::get_docker_logs(&ext_container, 200).await;

    // Only look at new log lines that appeared after we started streaming
    let new_logs = if logs_after.len() > baseline_len {
        &logs_after[baseline_len..]
    } else {
        &logs_after
    };

    let webhook_marker = format!("live_input.connected for input_id: {}", ck1_external_id);
    assert!(
        new_logs.contains(&webhook_marker),
        "Missing webhook for THIS custom key's Live Input.\n\
         Expected to find: '{}'\n\
         in new logs ({} bytes since baseline)",
        webhook_marker,
        new_logs.len(),
    );
    println!("[PASS] Step 7/{total_steps}: Webhook START received for input_id={}", ck1_external_id);

    // ── Step 8/11: LIVE Nostr event with custom metadata ──────────────
    println!("[TEST] Step 8/{total_steps}: LIVE Nostr event with custom metadata");
    let relay = NostrRelay::connect(&config.nostr_relay_url).await;
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - 600);

    // Poll for the event to appear — relay indexing can lag behind the webhook log
    let mut events = Vec::new();
    for attempt in 1..=6 {
        events = relay
            .query_30311_events(since, Some(&stream_id_1))
            .await;
        if !events.is_empty() {
            break;
        }
        println!(
            "  [WAIT] Attempt {}/6: no event with d-tag={} yet, retrying in 5s...",
            attempt, stream_id_1
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    assert!(
        !events.is_empty(),
        "No kind 30311 events with d-tag={} after 30s of polling",
        stream_id_1,
    );

    let live_event = events
        .iter()
        .find(|e| {
            nostr_relay::get_tag_value(e, "d").as_deref() == Some(stream_id_1.as_str())
                && nostr_relay::get_tag_value(e, "status").as_deref() == Some("live")
        })
        .expect(&format!(
            "No LIVE kind 30311 event with d-tag={} (found {} events, statuses: {:?})",
            stream_id_1,
            events.len(),
            events
                .iter()
                .map(|e| nostr_relay::get_tag_value(e, "status"))
                .collect::<Vec<_>>()
        ));
    assert!(
        nostr_relay::has_tag(live_event, "streaming"),
        "LIVE event missing 'streaming' tag"
    );

    let title = nostr_relay::get_tag_value(live_event, "title");
    assert_eq!(
        title.as_deref(),
        Some("Custom Key Test Stream"),
        "Title mismatch: {:?}",
        title
    );
    let summary = nostr_relay::get_tag_value(live_event, "summary");
    assert_eq!(
        summary.as_deref(),
        Some("E2E test of custom keys on external backend"),
        "Summary mismatch: {:?}",
        summary
    );

    let t_tags = nostr_relay::get_all_tag_values(live_event, "t");
    assert!(
        t_tags.contains(&"test".to_string()),
        "Missing 'test' t-tag (got {:?})",
        t_tags
    );
    assert!(
        t_tags.contains(&"custom-key".to_string()),
        "Missing 'custom-key' t-tag (got {:?})",
        t_tags
    );
    println!("[PASS] Step 8/{total_steps}: LIVE event with metadata verified");

    // ── Step 9/11: End custom key stream ──────────────────────────────
    println!("[TEST] Step 9/{total_steps}: End custom key stream");
    let logs_before_stop = docker::get_docker_logs(&ext_container, 200).await;
    let stop_baseline_len = logs_before_stop.len();

    ffmpeg.stop().await;
    tokio::time::sleep(Duration::from_secs(15)).await;
    let logs_after_stop = docker::get_docker_logs(&ext_container, 200).await;

    let new_stop_logs = if logs_after_stop.len() > stop_baseline_len {
        &logs_after_stop[stop_baseline_len..]
    } else {
        &logs_after_stop
    };

    let disconnect_marker = format!("live_input.disconnected for input_id: {}", ck1_external_id);
    assert!(
        new_stop_logs.contains(&disconnect_marker),
        "Missing disconnect webhook for THIS custom key's Live Input.\n\
         Expected to find: '{}'\n\
         in new logs ({} bytes since baseline)",
        disconnect_marker,
        new_stop_logs.len(),
    );
    println!("[PASS] Step 9/{total_steps}: Custom key stream ended (input_id={})", ck1_external_id);

    // ── Step 10/11: ENDED Nostr event ─────────────────────────────────
    println!("[TEST] Step 10/{total_steps}: ENDED Nostr event for custom key");
    let events = relay
        .query_30311_events(since, Some(&stream_id_1))
        .await;
    let ended_event = events
        .iter()
        .find(|e| {
            nostr_relay::get_tag_value(e, "d").as_deref() == Some(&stream_id_1)
                && nostr_relay::get_tag_value(e, "status").as_deref() == Some("ended")
        })
        .expect(&format!(
            "No ENDED event with d-tag={} (found {} events)",
            stream_id_1,
            events.len()
        ));

    assert!(
        nostr_relay::has_tag(ended_event, "ends"),
        "ENDED event missing 'ends' tag"
    );
    let ends_val = nostr_relay::get_tag_value(ended_event, "ends").unwrap();
    assert!(!ends_val.is_empty(), "'ends' tag is empty");

    let streaming_val = nostr_relay::get_tag_value(ended_event, "streaming");
    assert!(
        streaming_val.is_none() || streaming_val.as_deref() == Some(""),
        "ENDED event should not have 'streaming' tag (got {:?})",
        streaming_val
    );
    println!("[PASS] Step 10/{total_steps}: ENDED event verified");

    // ── Step 11/11: Keys persist after stream lifecycle ────────────────
    println!("[TEST] Step 11/{total_steps}: Keys persist after stream lifecycle");
    let keys_after = client.list_keys().await;
    let keys_after_arr = keys_after.as_array().expect("keys list is not an array");
    assert!(
        keys_after_arr.len() >= 2,
        "Expected at least 2 keys after lifecycle, got {}",
        keys_after_arr.len()
    );
    assert!(
        keys_after_arr
            .iter()
            .any(|k| k["key"].as_str() == Some(&key1)),
        "Key 1 disappeared after stream lifecycle"
    );
    assert!(
        keys_after_arr
            .iter()
            .any(|k| k["key"].as_str() == Some(&key2)),
        "Key 2 disappeared after stream lifecycle"
    );
    println!("[PASS] Step 11/{total_steps}: Keys persisted");

    relay.disconnect().await;
    println!("\n====== ALL {total_steps}/{total_steps} STEPS PASSED ======");
}
