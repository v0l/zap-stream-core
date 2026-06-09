mod common;

use common::api_client::ApiClient;
use common::config::TestConfig;
use common::db::TestDb;
use common::docker;
use common::ffmpeg::FfmpegStream;
use common::nostr_relay::{self, NostrRelay};
use nostr_sdk::{Keys, ToBech32, Timestamp};
use std::time::Duration;
use uuid::Uuid;

/// Stream to key 1 with metadata A, stream to key 2 with metadata B,
/// verify each Nostr event carries its own key's metadata and not the other's.
#[tokio::test]
#[ignore]
async fn e2e_custom_key_metadata_isolation() {
    let config = TestConfig::from_env();
    let total_steps = 13;

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
    let _db_container = docker::detect_container("db-1")
        .await
        .or(config.db_container.clone())
        .expect("Cannot find db container");
    println!("[PASS] Step 1/{total_steps}: Check prerequisites");

    let test_nsec = Keys::generate().secret_key().to_bech32().expect("bech32 nsec");
    let client = ApiClient::new(&test_nsec, &config.api_base_url()).await;
    let db = TestDb::connect(&config.db_connection_string()).await;
    db.ensure_user_exists(&client.pubkey_hex()).await;

    let account = client.get_account().await;
    let endpoints = account["endpoints"].as_array().expect("no endpoints");
    let rtmps = endpoints
        .iter()
        .find(|e| e["name"].as_str().unwrap_or("").starts_with("RTMPS-"))
        .expect("No RTMPS endpoint");
    let rtmp_url = rtmps["url"].as_str().unwrap();

    // ── Step 2: Create key 1 with Alpha metadata ───────────────────────
    let alpha_title = format!("Show Alpha {run_id}");
    let alpha_summary = format!("Alpha summary {run_id}");
    println!("[TEST] Step 2/{total_steps}: Create key 1 (Alpha)");
    let key1_resp = client
        .create_key(&alpha_title, &alpha_summary, &["alpha", run_id])
        .await;
    let key1 = key1_resp["key"]
        .as_str()
        .expect("No 'key' in response")
        .to_string();
    assert!(!key1.is_empty(), "Key 1 is empty");
    println!("[PASS] Step 2/{total_steps}: Key 1 created: {}...", &key1[..20.min(key1.len())]);

    // ── Step 3: Create key 2 with Beta metadata ────────────────────────
    let beta_title = format!("Show Beta {run_id}");
    let beta_summary = format!("Beta summary {run_id}");
    println!("[TEST] Step 3/{total_steps}: Create key 2 (Beta)");
    let key2_resp = client
        .create_key(&beta_title, &beta_summary, &["beta", run_id])
        .await;
    let key2 = key2_resp["key"]
        .as_str()
        .expect("No 'key' in response")
        .to_string();
    assert!(!key2.is_empty(), "Key 2 is empty");
    assert_ne!(key1, key2, "Key 1 and Key 2 are the same");
    println!("[PASS] Step 3/{total_steps}: Key 2 created: {}...", &key2[..20.min(key2.len())]);

    // ── Step 4: List keys, extract stream_ids ──────────────────────────
    println!("[TEST] Step 4/{total_steps}: List keys and extract stream_ids");
    let keys_list = client.list_keys().await;
    let keys_arr = keys_list.as_array().expect("keys list is not an array");

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

    let ck1_external_id = db
        .get_custom_key_external_id(&stream_id_1)
        .await
        .expect("No external_id for key 1");
    let ck2_external_id = db
        .get_custom_key_external_id(&stream_id_2)
        .await
        .expect("No external_id for key 2");
    assert_ne!(
        ck1_external_id, ck2_external_id,
        "Key 1 and Key 2 have the same Cloudflare external_id"
    );
    println!(
        "[PASS] Step 4/{total_steps}: stream_ids unique ({}, {}), CF inputs unique ({}, {})",
        &stream_id_1[..8],
        &stream_id_2[..8],
        &ck1_external_id[..8],
        &ck2_external_id[..8],
    );

    let relay = NostrRelay::connect(&config.nostr_relay_url).await;
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - 60);

    // ── Step 5: Stream to key 1 ────────────────────────────────────────
    println!("[TEST] Step 5/{total_steps}: Stream to key 1 (Alpha)");
    let stream1_start = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut ffmpeg1 = FfmpegStream::start_rtmps(rtmp_url, &key1, 90, 1000).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(ffmpeg1.is_running(), "FFmpeg died immediately for key 1");

    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs1 = docker::get_docker_logs_since(&ext_container, &stream1_start).await;
    let webhook_marker1 = format!("live_input.connected for input_id: {}", ck1_external_id);
    assert!(
        logs1.contains(&webhook_marker1),
        "Missing webhook for key 1's Live Input.\n\
         Expected: '{}'\n\
         in logs since {} ({} bytes)",
        webhook_marker1,
        stream1_start,
        logs1.len(),
    );
    println!("[PASS] Step 5/{total_steps}: Key 1 stream started, webhook received");

    // ── Step 6: Stop key 1 stream ──────────────────────────────────────
    println!("[TEST] Step 6/{total_steps}: Stop key 1 stream");
    ffmpeg1.stop().await;
    tokio::time::sleep(Duration::from_secs(15)).await;
    println!("[PASS] Step 6/{total_steps}: Key 1 stream stopped");

    // ── Step 7: Verify key 1 Nostr event has Alpha metadata ────────────
    println!("[TEST] Step 7/{total_steps}: Verify key 1 Nostr event has Alpha metadata");
    let events1 = relay
        .query_30311_events(since, Some(&stream_id_1))
        .await;
    assert!(
        !events1.is_empty(),
        "No kind 30311 events with d-tag={}",
        stream_id_1,
    );

    // Use the most recent event (ended or live) — it carries the same metadata
    let event1 = &events1[0];
    assert_eq!(
        nostr_relay::get_tag_value(event1, "d").as_deref(),
        Some(stream_id_1.as_str()),
        "Key 1 event d-tag mismatch"
    );

    let title1 = nostr_relay::get_tag_value(event1, "title");
    assert_eq!(
        title1.as_deref(),
        Some(alpha_title.as_str()),
        "Key 1 event has wrong title: {:?} (expected {:?})",
        title1,
        alpha_title,
    );
    let summary1 = nostr_relay::get_tag_value(event1, "summary");
    assert_eq!(
        summary1.as_deref(),
        Some(alpha_summary.as_str()),
        "Key 1 event has wrong summary: {:?} (expected {:?})",
        summary1,
        alpha_summary,
    );
    let t_tags1 = nostr_relay::get_all_tag_values(event1, "t");
    assert!(
        t_tags1.contains(&"alpha".to_string()),
        "Key 1 event missing 'alpha' t-tag (got {:?})",
        t_tags1,
    );
    assert!(
        t_tags1.contains(&run_id.to_string()),
        "Key 1 event missing run_id t-tag '{}' (got {:?})",
        run_id,
        t_tags1,
    );
    // Cross-contamination guard: key 1 must NOT have key 2's tag
    assert!(
        !t_tags1.contains(&"beta".to_string()),
        "Key 1 event has 'beta' t-tag — metadata cross-contamination! (got {:?})",
        t_tags1,
    );
    println!("[PASS] Step 7/{total_steps}: Key 1 event has Alpha metadata, no Beta contamination");

    // ── Step 8: Stream to key 2 ────────────────────────────────────────
    println!("[TEST] Step 8/{total_steps}: Stream to key 2 (Beta)");
    let stream2_start = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut ffmpeg2 = FfmpegStream::start_rtmps(rtmp_url, &key2, 90, 1000).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(ffmpeg2.is_running(), "FFmpeg died immediately for key 2");

    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs2 = docker::get_docker_logs_since(&ext_container, &stream2_start).await;
    let webhook_marker2 = format!("live_input.connected for input_id: {}", ck2_external_id);
    assert!(
        logs2.contains(&webhook_marker2),
        "Missing webhook for key 2's Live Input.\n\
         Expected: '{}'\n\
         in logs since {} ({} bytes)",
        webhook_marker2,
        stream2_start,
        logs2.len(),
    );
    println!("[PASS] Step 8/{total_steps}: Key 2 stream started, webhook received");

    // ── Step 9: Stop key 2 stream ──────────────────────────────────────
    println!("[TEST] Step 9/{total_steps}: Stop key 2 stream");
    ffmpeg2.stop().await;
    tokio::time::sleep(Duration::from_secs(15)).await;
    println!("[PASS] Step 9/{total_steps}: Key 2 stream stopped");

    // ── Step 10: Verify key 2 Nostr event has Beta metadata ────────────
    println!("[TEST] Step 10/{total_steps}: Verify key 2 Nostr event has Beta metadata");
    let events2 = relay
        .query_30311_events(since, Some(&stream_id_2))
        .await;
    assert!(
        !events2.is_empty(),
        "No kind 30311 events with d-tag={}",
        stream_id_2,
    );

    let event2 = &events2[0];
    assert_eq!(
        nostr_relay::get_tag_value(event2, "d").as_deref(),
        Some(stream_id_2.as_str()),
        "Key 2 event d-tag mismatch"
    );

    let title2 = nostr_relay::get_tag_value(event2, "title");
    assert_eq!(
        title2.as_deref(),
        Some(beta_title.as_str()),
        "Key 2 event has wrong title: {:?} (expected {:?})",
        title2,
        beta_title,
    );
    let summary2 = nostr_relay::get_tag_value(event2, "summary");
    assert_eq!(
        summary2.as_deref(),
        Some(beta_summary.as_str()),
        "Key 2 event has wrong summary: {:?} (expected {:?})",
        summary2,
        beta_summary,
    );
    let t_tags2 = nostr_relay::get_all_tag_values(event2, "t");
    assert!(
        t_tags2.contains(&"beta".to_string()),
        "Key 2 event missing 'beta' t-tag (got {:?})",
        t_tags2,
    );
    assert!(
        t_tags2.contains(&run_id.to_string()),
        "Key 2 event missing run_id t-tag '{}' (got {:?})",
        run_id,
        t_tags2,
    );
    // Cross-contamination guard: key 2 must NOT have key 1's tag
    assert!(
        !t_tags2.contains(&"alpha".to_string()),
        "Key 2 event has 'alpha' t-tag — metadata cross-contamination! (got {:?})",
        t_tags2,
    );
    println!("[PASS] Step 10/{total_steps}: Key 2 event has Beta metadata, no Alpha contamination");

    // ── Step 11: Verify both events have correct lifecycle tags ─────────
    println!("[TEST] Step 11/{total_steps}: Verify lifecycle tags on both events");
    let final_events1 = relay
        .query_30311_events(since, Some(&stream_id_1))
        .await;
    let ended1 = final_events1
        .iter()
        .find(|e| nostr_relay::get_tag_value(e, "status").as_deref() == Some("ended"))
        .expect(&format!(
            "No ENDED event for key 1 (d-tag={})",
            stream_id_1
        ));
    assert!(
        nostr_relay::has_tag(ended1, "ends"),
        "Key 1 ENDED event missing 'ends' tag"
    );

    let final_events2 = relay
        .query_30311_events(since, Some(&stream_id_2))
        .await;
    let ended2 = final_events2
        .iter()
        .find(|e| nostr_relay::get_tag_value(e, "status").as_deref() == Some("ended"))
        .expect(&format!(
            "No ENDED event for key 2 (d-tag={})",
            stream_id_2
        ));
    assert!(
        nostr_relay::has_tag(ended2, "ends"),
        "Key 2 ENDED event missing 'ends' tag"
    );
    println!("[PASS] Step 11/{total_steps}: Both streams have ENDED events with 'ends' tag");

    // ── Step 12: Cloudflare API validation (both keys) ─────────────────
    println!("[TEST] Step 12/{total_steps}: Cloudflare API validation");
    if let (Some(cf_token), Some(cf_account)) =
        (&config.cloudflare_api_token, &config.cloudflare_account_id)
    {
        let http = reqwest::Client::new();
        for (label, ext_id) in [("Key 1", &ck1_external_id), ("Key 2", &ck2_external_id)] {
            let cf_url = format!(
                "https://api.cloudflare.com/client/v4/accounts/{}/stream/live_inputs/{}",
                cf_account, ext_id
            );
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
                "{} Cloudflare API returned success=false for input {}",
                label,
                ext_id,
            );
        }
        println!("[PASS] Step 12/{total_steps}: Both Cloudflare Live Inputs validated");
    } else {
        println!(
            "[PASS] Step 12/{total_steps}: Cloudflare API validation skipped (no credentials)"
        );
    }

    // ── Step 13: Keys persist after full lifecycle ──────────────────────
    println!("[TEST] Step 13/{total_steps}: Keys persist after lifecycle");
    let keys_after = client.list_keys().await;
    let keys_after_arr = keys_after.as_array().expect("keys list is not an array");
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
    println!("[PASS] Step 13/{total_steps}: Both keys persisted");

    relay.disconnect().await;
    println!("\n====== ALL {total_steps}/{total_steps} STEPS PASSED ======");
}
