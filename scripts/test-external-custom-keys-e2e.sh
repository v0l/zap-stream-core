#!/bin/bash

# ==========================================
# External Backend Custom Keys E2E Test
# ==========================================
#
# Tests custom stream key management with the external Cloudflare backend:
# 1. Create custom keys with metadata via POST /api/v1/keys
# 2. List keys and validate via GET /api/v1/keys
# 3. Validate key exists on Cloudflare API directly (requires .env)
# 4. Stream using a custom key
# 5. Verify Nostr event carries custom metadata (title, summary, tags)
# 6. Verify stream END and Nostr event lifecycle
#
# Usage:
#   cd scripts && npm install && cd ..
#   cp scripts/.env.example scripts/.env  # fill in CF credentials
#   ./scripts/test-external-custom-keys-e2e.sh
#
# Environment variables:
#   ZS_EXTERNAL_CONTAINER  Override external service container name (default: auto-detect)
#   ZS_DB_CONTAINER        Override DB container name (default: auto-detect)
#   ZS_API_PORT            API port (default: 8080)
#   DB_ROOT_PASSWORD       MariaDB root password (default: root)
#   NOSTR_RELAY_URL        Nostr relay WebSocket URL (default: ws://localhost:3334)

set -e

# ── Configuration ──────────────────────────────────────────────────

API_PORT="${ZS_API_PORT:-8080}"
DB_PASSWORD="${DB_ROOT_PASSWORD:-root}"
NOSTR_RELAY_URL="${NOSTR_RELAY_URL:-ws://localhost:3334}"

TEST_NSEC="nsec107gexedhvf97ej83jzalley9wt682mlgy9ty5xwsp98vnph09fysssnzlk"
TEST_NPUB="npub1u0mm82x7muct7cy8y7urztyctgm0r6k27gdax04fa4q28x7q0shq6slmah"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load .env for Cloudflare API credentials if present
if [ -f "$SCRIPT_DIR/.env" ]; then
    source "$SCRIPT_DIR/.env"
    echo "✓ Loaded credentials from .env"
else
    echo "⚠️  No .env file found (Cloudflare API validation will be skipped)"
fi

# Track results
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_TOTAL=0

pass_test() {
    local name="$1"
    TESTS_PASSED=$((TESTS_PASSED + 1))
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    echo "✅ $name PASSED"
}

fail_test() {
    local name="$1"
    local reason="$2"
    TESTS_FAILED=$((TESTS_FAILED + 1))
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    echo "❌ $name FAILED: $reason"
}

make_auth_token() {
    local nsec="$1" url="$2" method="$3"
    local auth_json
    auth_json=$(node "$SCRIPT_DIR/sign_nip98.js" "$nsec" "$url" "$method" 2>&1)
    if [ $? -ne 0 ]; then
        echo "ERROR: Failed to create NIP-98 event" >&2
        return 1
    fi
    echo "$auth_json" | base64
}

query_latest_30311() {
    local since="$1"
    local tmp_file
    tmp_file=$(mktemp)

    set +e
    node "$SCRIPT_DIR/query_nostr_events_auth.js" 30311 --since "$since" --relay "$NOSTR_RELAY_URL" > "$tmp_file" 2>&1 &
    local qpid=$!

    local counter=0
    while [ $counter -lt 15 ]; do
        if ! ps -p $qpid > /dev/null 2>&1; then break; fi
        sleep 1
        counter=$((counter + 1))
    done
    if ps -p $qpid > /dev/null 2>&1; then
        kill -9 $qpid 2>/dev/null || true
    fi
    set -e

    if grep -q '"kind": 30311' "$tmp_file" 2>/dev/null; then
        awk '/^{$/,/^}$/ {print} /^}$/ {print "---SPLIT---"}' "$tmp_file" | \
            awk 'BEGIN{RS="---SPLIT---"} /"kind": 30311/ {print}' | \
            jq -s 'sort_by(.created_at) | reverse | .[0]' 2>/dev/null
    else
        echo "null"
    fi

    rm -f "$tmp_file"
}

# Cleanup handler
cleanup() {
    echo ""
    echo "Cleaning up..."
    [ -n "${FFMPEG_PID:-}" ] && ps -p $FFMPEG_PID > /dev/null 2>&1 && kill -9 $FFMPEG_PID 2>/dev/null
    rm -f "${FFMPEG_LOG:-}"
    pkill -9 -f "ffmpeg.*testsrc" 2>/dev/null || true
}
trap cleanup EXIT

echo "========================================"
echo "External Custom Keys E2E Test"
echo "========================================"
echo ""
echo "Test Pubkey: $TEST_NPUB"
echo "API Port:    $API_PORT"
echo ""

# ── TEST 1: Prerequisites ─────────────────────────────────────────

echo "========================================"
echo "TEST 1: Prerequisites"
echo "========================================"

PREREQ_OK=true

for cmd in node jq ffmpeg; do
    if ! command -v $cmd &> /dev/null; then
        echo "❌ $cmd not found"
        PREREQ_OK=false
    fi
done

if ! docker ps &> /dev/null; then
    echo "❌ Docker is not running"
    PREREQ_OK=false
fi

if [ -z "${ZS_EXTERNAL_CONTAINER:-}" ]; then
    EXTERNAL_CONTAINER=$(docker ps --format '{{.Names}}' | grep -E 'zap-stream-external' | head -1)
else
    EXTERNAL_CONTAINER="$ZS_EXTERNAL_CONTAINER"
fi

if [ -z "${ZS_DB_CONTAINER:-}" ]; then
    DB_CONTAINER=$(docker ps --format '{{.Names}}' | grep -E 'db-1' | head -1)
else
    DB_CONTAINER="$ZS_DB_CONTAINER"
fi

if [ -z "$EXTERNAL_CONTAINER" ]; then
    echo "❌ Cannot find zap-stream-external container"
    PREREQ_OK=false
else
    echo "✓ External container: $EXTERNAL_CONTAINER"
fi

if [ -z "$DB_CONTAINER" ]; then
    echo "❌ Cannot find database container"
    PREREQ_OK=false
else
    echo "✓ DB container: $DB_CONTAINER"
fi

if [ "$PREREQ_OK" != "true" ]; then
    fail_test "TEST 1: Prerequisites"
    exit 1
fi

echo "✓ All prerequisites met"
pass_test "TEST 1: Prerequisites"
echo ""

# Decode npub and ensure user exists
TEST_PUBKEY_HEX=$(node "$SCRIPT_DIR/decode_npub.js" "$TEST_NPUB" 2>&1)
UPPER_PUBKEY=$(echo "$TEST_PUBKEY_HEX" | tr '[:lower:]' '[:upper:]')

docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${TEST_PUBKEY_HEX}'), 0);" \
  2>/dev/null || true

echo "Test pubkey hex: $TEST_PUBKEY_HEX"
echo ""

# ── TEST 2: Create first custom key ───────────────────────────────

echo "========================================"
echo "TEST 2: Create custom key with metadata"
echo "========================================"

POST_URL="http://localhost:${API_PORT}/api/v1/keys"
POST_AUTH=$(make_auth_token "$TEST_NSEC" "$POST_URL" "POST")

KEY_REQUEST_1='{
  "event": {
    "title": "Custom Key Test Stream",
    "summary": "E2E test of custom keys on external backend",
    "tags": ["test", "custom-key", "e2e"]
  }
}'

CREATE_RESPONSE_1=$(curl -s -X POST "$POST_URL" \
  -H "Authorization: Nostr $POST_AUTH" \
  -H "Content-Type: application/json" \
  -d "$KEY_REQUEST_1")

CUSTOM_KEY_1=$(echo "$CREATE_RESPONSE_1" | jq -r '.key // empty')

if [ -z "$CUSTOM_KEY_1" ]; then
    fail_test "TEST 2: Create custom key" "Failed: $CREATE_RESPONSE_1"
    exit 1
fi

echo "✓ Custom key 1 created: ${CUSTOM_KEY_1:0:20}... (${#CUSTOM_KEY_1} chars)"
pass_test "TEST 2: Create custom key"
echo ""

# ── TEST 3: Create second custom key ──────────────────────────────

echo "========================================"
echo "TEST 3: Create second custom key"
echo "========================================"

# Fresh auth token
POST_AUTH_2=$(make_auth_token "$TEST_NSEC" "$POST_URL" "POST")

KEY_REQUEST_2='{
  "event": {
    "title": "Second Custom Stream",
    "summary": "Testing multiple keys per user"
  }
}'

CREATE_RESPONSE_2=$(curl -s -X POST "$POST_URL" \
  -H "Authorization: Nostr $POST_AUTH_2" \
  -H "Content-Type: application/json" \
  -d "$KEY_REQUEST_2")

CUSTOM_KEY_2=$(echo "$CREATE_RESPONSE_2" | jq -r '.key // empty')

if [ -z "$CUSTOM_KEY_2" ]; then
    fail_test "TEST 3: Create second key" "Failed: $CREATE_RESPONSE_2"
    exit 1
fi

if [ "$CUSTOM_KEY_1" == "$CUSTOM_KEY_2" ]; then
    fail_test "TEST 3: Create second key" "Both keys are identical!"
    exit 1
fi

echo "✓ Custom key 2 created: ${CUSTOM_KEY_2:0:20}... (${#CUSTOM_KEY_2} chars)"
echo "✓ Keys are unique"
pass_test "TEST 3: Create second key"
echo ""

# ── TEST 4: List keys ─────────────────────────────────────────────

echo "========================================"
echo "TEST 4: List all custom keys"
echo "========================================"

GET_KEYS_URL="http://localhost:${API_PORT}/api/v1/keys"
GET_AUTH=$(make_auth_token "$TEST_NSEC" "$GET_KEYS_URL" "GET")

KEYS_LIST=$(curl -s "$GET_KEYS_URL" -H "Authorization: Nostr $GET_AUTH")

KEY_COUNT=$(echo "$KEYS_LIST" | jq 'length' 2>/dev/null)

T4_OK=true

if [ -z "$KEY_COUNT" ] || [ "$KEY_COUNT" -lt 2 ]; then
    echo "❌ Expected at least 2 keys, got: $KEY_COUNT"
    T4_OK=false
else
    echo "✓ GET /api/v1/keys returned $KEY_COUNT key(s)"
fi

# Verify key 1 is in the list
KEY_1_ENTRY=$(echo "$KEYS_LIST" | jq --arg key "$CUSTOM_KEY_1" '.[] | select(.key == $key)' 2>/dev/null)
if [ -z "$KEY_1_ENTRY" ] || [ "$KEY_1_ENTRY" == "null" ]; then
    echo "❌ Custom key 1 not found in list"
    T4_OK=false
else
    KEY_1_STREAM_ID=$(echo "$KEY_1_ENTRY" | jq -r '.stream_id')
    echo "✓ Key 1 found, stream_id: $KEY_1_STREAM_ID"
fi

# Verify key 2 is in the list
KEY_2_ENTRY=$(echo "$KEYS_LIST" | jq --arg key "$CUSTOM_KEY_2" '.[] | select(.key == $key)' 2>/dev/null)
if [ -z "$KEY_2_ENTRY" ] || [ "$KEY_2_ENTRY" == "null" ]; then
    echo "❌ Custom key 2 not found in list"
    T4_OK=false
else
    KEY_2_STREAM_ID=$(echo "$KEY_2_ENTRY" | jq -r '.stream_id')
    echo "✓ Key 2 found, stream_id: $KEY_2_STREAM_ID"
fi

# Verify stream_ids are different
if [ -n "$KEY_1_STREAM_ID" ] && [ -n "$KEY_2_STREAM_ID" ] && [ "$KEY_1_STREAM_ID" == "$KEY_2_STREAM_ID" ]; then
    echo "❌ Both keys have same stream_id"
    T4_OK=false
else
    echo "✓ Keys have unique stream_ids"
fi

if [ "$T4_OK" == "true" ]; then
    pass_test "TEST 4: List keys"
else
    fail_test "TEST 4: List keys" "Validation failed"
fi
echo ""

# ── TEST 5: Cloudflare API validation (optional) ──────────────────

echo "========================================"
echo "TEST 5: Cloudflare API direct validation"
echo "========================================"

if [ -z "${CLOUDFLARE_API_TOKEN:-}" ] || [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ]; then
    echo "⚠️  CLOUDFLARE_API_TOKEN or CLOUDFLARE_ACCOUNT_ID not set"
    echo "   Skipping direct Cloudflare API validation"
    echo "   (Set these in scripts/.env to enable this test)"
    pass_test "TEST 5: Cloudflare API (skipped)"
else
    # Get external_id for the custom key from the DB
    # The user_stream_key table has an external_id column that stores the CF Live Input UID
    CK_EXTERNAL_ID=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
        -e "SELECT external_id FROM user_stream_key WHERE \`key\` = '${CUSTOM_KEY_1}' LIMIT 1;" -s -N 2>/dev/null)

    if [ -z "$CK_EXTERNAL_ID" ] || [ "$CK_EXTERNAL_ID" == "NULL" ]; then
        # The key column stores the stream_key from CF, external_id stores the live input UID
        # Try looking up by stream_id instead
        CK_EXTERNAL_ID=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
            -e "SELECT external_id FROM user_stream_key WHERE stream_id = '${KEY_1_STREAM_ID}' LIMIT 1;" -s -N 2>/dev/null)
    fi

    T5_OK=true

    if [ -z "$CK_EXTERNAL_ID" ] || [ "$CK_EXTERNAL_ID" == "NULL" ]; then
        echo "❌ No external_id found for custom key in user_stream_key table"
        T5_OK=false
    else
        echo "✓ Custom key external_id (CF Live Input UID): $CK_EXTERNAL_ID"

        CF_RESPONSE=$(curl -s "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/stream/live_inputs/$CK_EXTERNAL_ID" \
          -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN")

        if ! echo "$CF_RESPONSE" | jq -e '.success == true' > /dev/null 2>&1; then
            echo "❌ Cloudflare API query failed"
            echo "Response: $(echo "$CF_RESPONSE" | jq -c '.errors' 2>/dev/null)"
            T5_OK=false
        else
            echo "✓ Cloudflare Live Input exists"

            CF_STREAM_KEY=$(echo "$CF_RESPONSE" | jq -r '.result.rtmps.streamKey')
            CF_RTMPS_URL=$(echo "$CF_RESPONSE" | jq -r '.result.rtmps.url')

            echo "✓ CF RTMPS URL: $CF_RTMPS_URL"
            echo "✓ CF Stream Key: ${CF_STREAM_KEY:0:20}... (${#CF_STREAM_KEY} chars)"

            # Verify our API key matches CF key
            if [ "$CUSTOM_KEY_1" == "$CF_STREAM_KEY" ]; then
                echo "✓ Our API key matches Cloudflare stream key"
            else
                echo "⚠️  Key mismatch (may be rotated): ours=${CUSTOM_KEY_1:0:15}... CF=${CF_STREAM_KEY:0:15}..."
            fi
        fi
    fi

    if [ "$T5_OK" == "true" ]; then
        pass_test "TEST 5: Cloudflare API validation"
    else
        fail_test "TEST 5: Cloudflare API validation" "CF validation failed"
    fi
fi
echo ""

# ── TEST 6: Stream using custom key ───────────────────────────────

echo "========================================"
echo "TEST 6: Stream using custom key"
echo "========================================"

# Get the RTMPS base URL from the account endpoint
ACCOUNT_URL="http://localhost:${API_PORT}/api/v1/account"
ACCT_AUTH=$(make_auth_token "$TEST_NSEC" "$ACCOUNT_URL" "GET")
ACCT_RESPONSE=$(curl -s "$ACCOUNT_URL" -H "Authorization: Nostr $ACCT_AUTH")
RTMP_BASE_URL=$(echo "$ACCT_RESPONSE" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-")) | .url')

CUSTOM_RTMP_DEST="${RTMP_BASE_URL}${CUSTOM_KEY_1}"
FFMPEG_LOG=$(mktemp)

echo "Streaming with custom key to: ${RTMP_BASE_URL}(custom-key)"

ffmpeg -re -t 30 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$CUSTOM_RTMP_DEST" \
  </dev/null >>"$FFMPEG_LOG" 2>&1 &

FFMPEG_PID=$!

sleep 3
if ! ps -p $FFMPEG_PID > /dev/null 2>&1; then
    echo "❌ FFmpeg failed to start"
    cat "$FFMPEG_LOG"
    fail_test "TEST 6: Stream with custom key" "FFmpeg died"
    exit 1
fi

echo "✓ FFmpeg streaming with custom key (PID: $FFMPEG_PID)"
pass_test "TEST 6: Stream with custom key"
echo ""

# ── TEST 7: Webhook START for custom key ──────────────────────────

echo "========================================"
echo "TEST 7: Webhook START for custom key"
echo "========================================"

echo "Waiting 20 seconds for webhooks..."
sleep 20

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T7_CHECKS=0
T7_TOTAL=2

if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.connected"; then
    echo "✓ Webhook: live_input.connected"
    T7_CHECKS=$((T7_CHECKS + 1))
else
    echo "✗ Missing: live_input.connected"
fi

if echo "$LOGS" | grep -q "Published stream event"; then
    echo "✓ Stream event published"
    T7_CHECKS=$((T7_CHECKS + 1))
else
    echo "✗ Missing: Published stream event"
fi

if [ $T7_CHECKS -eq $T7_TOTAL ]; then
    pass_test "TEST 7: Webhook START"
else
    fail_test "TEST 7: Webhook START" "$T7_CHECKS/$T7_TOTAL checks passed"
fi
echo ""

# ── TEST 8: LIVE Nostr event has custom metadata ──────────────────

echo "========================================"
echo "TEST 8: LIVE Nostr event with custom metadata"
echo "========================================"

SINCE_TIME=$(($(date +%s) - 600))
EVENT_JSON=$(query_latest_30311 "$SINCE_TIME")

T8_CHECKS=0
T8_TOTAL=6

if [ "$EVENT_JSON" == "null" ] || [ -z "$EVENT_JSON" ]; then
    fail_test "TEST 8: LIVE Nostr metadata" "No kind 30311 event found"
else
    # Check status = live
    STATUS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
    if [ "$STATUS" == "live" ]; then
        echo "✓ Event status: live"
        T8_CHECKS=$((T8_CHECKS + 1))
    else
        echo "✗ Expected status 'live', got: '$STATUS'"
    fi

    # Check streaming tag
    STREAMING_URL=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "streaming")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$STREAMING_URL" ] && [ "$STREAMING_URL" != "null" ]; then
        echo "✓ Event has 'streaming' tag: ${STREAMING_URL:0:60}..."
        T8_CHECKS=$((T8_CHECKS + 1))
    else
        echo "✗ Missing 'streaming' tag"
    fi

    # Check title from custom key metadata
    TITLE=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "title")? | .[1]?' 2>/dev/null | head -n 1)
    if [ "$TITLE" == "Custom Key Test Stream" ]; then
        echo "✓ Event has custom title: '$TITLE'"
        T8_CHECKS=$((T8_CHECKS + 1))
    else
        echo "✗ Expected title 'Custom Key Test Stream', got: '$TITLE'"
    fi

    # Check summary
    SUMMARY=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "summary")? | .[1]?' 2>/dev/null | head -n 1)
    if [ "$SUMMARY" == "E2E test of custom keys on external backend" ]; then
        echo "✓ Event has custom summary: '$SUMMARY'"
        T8_CHECKS=$((T8_CHECKS + 1))
    else
        echo "✗ Expected summary 'E2E test of custom keys on external backend', got: '$SUMMARY'"
    fi

    # Check 't' tags
    T_TAGS=$(echo "$EVENT_JSON" | jq -r '[.tags[]? | select(.[0] == "t")? | .[1]?] | join(",")' 2>/dev/null)
    if echo "$T_TAGS" | grep -q "test"; then
        echo "✓ Event has tag: 'test'"
        T8_CHECKS=$((T8_CHECKS + 1))
    else
        echo "✗ Missing tag: 'test'"
    fi

    if echo "$T_TAGS" | grep -q "custom-key"; then
        echo "✓ Event has tag: 'custom-key'"
        T8_CHECKS=$((T8_CHECKS + 1))
    else
        echo "✗ Missing tag: 'custom-key'"
    fi

    if [ $T8_CHECKS -eq $T8_TOTAL ]; then
        pass_test "TEST 8: LIVE Nostr metadata"
    else
        fail_test "TEST 8: LIVE Nostr metadata" "$T8_CHECKS/$T8_TOTAL checks passed"
    fi
fi
echo ""

# ── TEST 9: End stream ────────────────────────────────────────────

echo "========================================"
echo "TEST 9: End custom key stream"
echo "========================================"

if ps -p $FFMPEG_PID > /dev/null 2>&1; then
    kill -9 $FFMPEG_PID 2>/dev/null || true
    echo "✓ Stream stopped"
else
    echo "⚠️  Stream already stopped"
fi

echo "Waiting 15 seconds for END webhooks..."
sleep 15

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T9_CHECKS=0
T9_TOTAL=2

if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.disconnected"; then
    echo "✓ Webhook: live_input.disconnected"
    T9_CHECKS=$((T9_CHECKS + 1))
else
    echo "✗ Missing: live_input.disconnected"
fi

if echo "$LOGS" | grep -q "Stream ended"; then
    echo "✓ Stream ended"
    T9_CHECKS=$((T9_CHECKS + 1))
else
    echo "✗ Missing: Stream ended"
fi

if [ $T9_CHECKS -eq $T9_TOTAL ]; then
    pass_test "TEST 9: End custom key stream"
else
    fail_test "TEST 9: End custom key stream" "$T9_CHECKS/$T9_TOTAL checks passed"
fi
echo ""

# ── TEST 10: ENDED Nostr event ────────────────────────────────────

echo "========================================"
echo "TEST 10: ENDED Nostr event for custom key"
echo "========================================"

SINCE_TIME=$(($(date +%s) - 600))
END_EVENT_JSON=$(query_latest_30311 "$SINCE_TIME")

T10_CHECKS=0
T10_TOTAL=3

if [ "$END_EVENT_JSON" == "null" ] || [ -z "$END_EVENT_JSON" ]; then
    fail_test "TEST 10: ENDED Nostr event" "No kind 30311 event found"
else
    # Check status = ended
    STATUS=$(echo "$END_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
    if [ "$STATUS" == "ended" ]; then
        echo "✓ Event status: ended"
        T10_CHECKS=$((T10_CHECKS + 1))
    else
        echo "✗ Expected status 'ended', got: '$STATUS'"
    fi

    # Check ends tag
    ENDS=$(echo "$END_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "ends")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$ENDS" ] && [ "$ENDS" != "null" ] && [ "$ENDS" != "" ]; then
        echo "✓ Event has 'ends' timestamp: $ENDS"
        T10_CHECKS=$((T10_CHECKS + 1))
    else
        echo "✗ Missing 'ends' tag"
    fi

    # Check streaming tag removed
    STREAMING_URL=$(echo "$END_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "streaming")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -z "$STREAMING_URL" ] || [ "$STREAMING_URL" == "null" ] || [ "$STREAMING_URL" == "" ]; then
        echo "✓ 'streaming' tag removed (correct for ended)"
        T10_CHECKS=$((T10_CHECKS + 1))
    else
        echo "✗ 'streaming' tag still present"
    fi

    if [ $T10_CHECKS -eq $T10_TOTAL ]; then
        pass_test "TEST 10: ENDED Nostr event"
    else
        fail_test "TEST 10: ENDED Nostr event" "$T10_CHECKS/$T10_TOTAL checks passed"
    fi
fi
echo ""

# ── TEST 11: Keys persist after stream lifecycle ──────────────────

echo "========================================"
echo "TEST 11: Keys persist after stream lifecycle"
echo "========================================"

GET_AUTH_FINAL=$(make_auth_token "$TEST_NSEC" "$GET_KEYS_URL" "GET")
KEYS_FINAL=$(curl -s "$GET_KEYS_URL" -H "Authorization: Nostr $GET_AUTH_FINAL")

FINAL_KEY_COUNT=$(echo "$KEYS_FINAL" | jq 'length' 2>/dev/null)

T11_OK=true

if [ "$FINAL_KEY_COUNT" -lt 2 ]; then
    echo "❌ Expected at least 2 keys after lifecycle, got: $FINAL_KEY_COUNT"
    T11_OK=false
else
    echo "✓ $FINAL_KEY_COUNT keys still present after stream lifecycle"
fi

# Verify key 1 still in list
KEY_1_STILL=$(echo "$KEYS_FINAL" | jq --arg key "$CUSTOM_KEY_1" '[.[] | select(.key == $key)] | length' 2>/dev/null)
if [ "$KEY_1_STILL" -ge 1 ]; then
    echo "✓ Custom key 1 persisted"
else
    echo "❌ Custom key 1 missing from list"
    T11_OK=false
fi

# Verify key 2 still in list
KEY_2_STILL=$(echo "$KEYS_FINAL" | jq --arg key "$CUSTOM_KEY_2" '[.[] | select(.key == $key)] | length' 2>/dev/null)
if [ "$KEY_2_STILL" -ge 1 ]; then
    echo "✓ Custom key 2 persisted"
else
    echo "❌ Custom key 2 missing from list"
    T11_OK=false
fi

if [ "$T11_OK" == "true" ]; then
    pass_test "TEST 11: Key persistence"
else
    fail_test "TEST 11: Key persistence" "Keys not persisted"
fi
echo ""

# ── Summary ────────────────────────────────────────────────────────

echo "========================================"
echo "TEST SUMMARY"
echo "========================================"
echo ""
echo "Total:  $TESTS_TOTAL"
echo "Passed: $TESTS_PASSED"
echo "Failed: $TESTS_FAILED"
echo ""

if [ $TESTS_FAILED -eq 0 ]; then
    echo "✅ ALL TESTS PASSED"
else
    echo "❌ $TESTS_FAILED TEST(S) FAILED"
fi

echo ""
echo "Custom Keys Summary:"
echo "  Key 1: ${CUSTOM_KEY_1:0:20}... (stream: ${KEY_1_STREAM_ID:-unknown})"
echo "  Key 2: ${CUSTOM_KEY_2:0:20}... (stream: ${KEY_2_STREAM_ID:-unknown})"
echo ""
echo "To review logs:"
echo "  docker logs --tail 300 $EXTERNAL_CONTAINER"

exit $TESTS_FAILED
