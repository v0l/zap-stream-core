mod common;

use common::api_client::ApiClient;
use common::config::TestConfig;
use common::db::TestDb;

/// Safe test-only nsec (same as e2e_single_user / e2e_custom_keys).
const TEST_NSEC: &str = "nsec107gexedhvf97ej83jzalley9wt682mlgy9ty5xwsp98vnph09fysssnzlk";

/// Smoke test: POST /keys should create a custom stream key with its own
/// Cloudflare Live Input, returning a key and stream_id.  We then verify
/// the DB has an external_id (Cloudflare UUID) for that key.
///
/// This isolates UUID generation at the keys-endpoint level.
#[tokio::test]
#[ignore]
async fn key_creation_generates_cloudflare_uuid() {
    let config = TestConfig::from_env();
    let db = TestDb::connect(&config.db_connection_string()).await;
    let client = ApiClient::new(TEST_NSEC, &config.api_base_url()).await;

    // Ensure user exists in DB
    db.ensure_user_exists(&client.pubkey_hex()).await;

    // Ensure account is provisioned (creates primary Live Input if needed)
    let _account = client.get_account().await;

    // Create a custom key
    let marker = format!("smoke-key-{}", chrono::Utc::now().timestamp());
    println!("[SMOKE] Creating key with title: {}", marker);

    let resp = client
        .create_key(&marker, "Smoke test for key UUID generation", &["smoke"])
        .await;
    println!("[SMOKE] POST /keys response: {}", resp);

    // Verify response has a key
    let key = resp["key"]
        .as_str()
        .expect("Response missing 'key' field");
    assert!(!key.is_empty(), "'key' is empty");
    println!("[SMOKE] Key: {}...", &key[..20.min(key.len())]);

    // List keys to get stream_id for our new key
    let keys_list = client.list_keys().await;
    let keys_arr = keys_list
        .as_array()
        .expect("GET /keys response is not an array");

    let entry = keys_arr
        .iter()
        .find(|k| k["key"].as_str() == Some(key))
        .expect("Created key not found in list");

    let stream_id = entry["stream_id"]
        .as_str()
        .expect("Key entry missing 'stream_id'");
    assert!(!stream_id.is_empty(), "stream_id is empty");
    println!("[SMOKE] stream_id: {}", stream_id);

    // Verify DB has an external_id for this custom key
    let ext_id = db.get_custom_key_external_id(stream_id).await;
    println!("[SMOKE] DB external_id for stream_id {}: {:?}", stream_id, ext_id);

    assert!(
        ext_id.is_some(),
        "No external_id in DB for custom key stream_id={} — Cloudflare provisioning failed",
        stream_id
    );
    let uuid = ext_id.unwrap();
    assert!(!uuid.is_empty(), "external_id is empty string");
    println!(
        "[PASS] Key creation generated Cloudflare UUID: {} (stream_id={})",
        uuid, stream_id
    );
}
