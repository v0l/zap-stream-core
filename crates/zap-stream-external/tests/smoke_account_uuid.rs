mod common;

use common::api_client::ApiClient;
use common::config::TestConfig;
use common::db::TestDb;
use nostr_sdk::{Keys, ToBech32};

/// Smoke test: a brand-new nsec calling GET /account should create a Cloudflare
/// Live Input and return endpoints containing the external_id (UUID).
///
/// This isolates UUID generation at the accounts-endpoint level.
#[tokio::test]
#[ignore]
async fn account_creation_generates_cloudflare_uuid() {
    let config = TestConfig::from_env();
    let db = TestDb::connect(&config.db_connection_string()).await;

    // Generate a completely fresh keypair — no prior DB state
    let fresh_keys = Keys::generate();
    let fresh_nsec = fresh_keys.secret_key().to_bech32().expect("bech32 nsec");
    let pubkey_hex = fresh_keys.public_key().to_hex();
    println!("[SMOKE] Fresh pubkey: {}", pubkey_hex);

    // Ensure user row exists (the API requires it)
    db.ensure_user_exists(&pubkey_hex).await;

    // Verify no external_id yet
    let before = db.get_external_id(&pubkey_hex).await;
    println!("[SMOKE] external_id before GET /account: {:?}", before);
    assert!(
        before.is_none(),
        "Fresh user already has an external_id — test is not isolated"
    );

    // Call GET /account — this should provision a Cloudflare Live Input
    let client = ApiClient::new(&fresh_nsec, &config.api_base_url()).await;
    let account = client.get_account().await;
    println!("[SMOKE] GET /account response: {}", account);

    // Verify endpoints array exists and has at least one entry
    let endpoints = account["endpoints"]
        .as_array()
        .expect("response missing 'endpoints' array");
    assert!(
        !endpoints.is_empty(),
        "endpoints array is empty — no Cloudflare Live Input created"
    );

    // Each endpoint should have a non-empty URL
    for ep in endpoints {
        let name = ep["name"].as_str().unwrap_or("<unnamed>");
        let url = ep["url"].as_str().unwrap_or("");
        println!("[SMOKE] Endpoint: {} -> {}", name, url);
        assert!(!url.is_empty(), "Endpoint '{}' has empty URL", name);
    }

    // Verify DB now has an external_id (the Cloudflare UUID)
    let after = db.get_external_id(&pubkey_hex).await;
    println!("[SMOKE] external_id after GET /account: {:?}", after);
    assert!(
        after.is_some(),
        "DB still has no external_id after GET /account — Cloudflare provisioning failed"
    );
    let uuid = after.unwrap();
    assert!(
        !uuid.is_empty(),
        "external_id is empty string"
    );
    assert!(
        uuid.len() == 32 || uuid.len() == 36,
        "external_id '{}' doesn't look like a UUID (len={})",
        uuid,
        uuid.len()
    );

    println!("[PASS] Account creation generated Cloudflare UUID: {}", uuid);
}
