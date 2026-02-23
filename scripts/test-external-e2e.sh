#!/bin/bash

# ==========================================
# External Backend (Cloudflare) E2E Integration Test
# ==========================================
#
# Tests the full streaming lifecycle using the zap-stream-external
# service with Cloudflare Stream as the ingest backend.
#
# Usage:
#   cd scripts && npm install && cd ..
#   # Start external stack (docker-compose.external.yaml)
#   ./scripts/test-external-e2e.sh
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

# Test credentials (safe test keypair, not production)
TEST_NSEC="nsec107gexedhvf97ej83jzalley9wt682mlgy9ty5xwsp98vnph09fysssnzlk"
TEST_NPUB="npub1u0mm82x7muct7cy8y7urztyctgm0r6k27gdax04fa4q28x7q0shq6slmah"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

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

# Helper: create NIP-98 auth token for a given URL and method
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

# Helper: query Nostr relay for most recent kind 30311 event
# Usage: query_latest_30311 <since_timestamp> [d_tag_filter]
query_latest_30311() {
    local since="$1"
    local d_filter="${2:-}"
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

    # Parse all events and return most recent by created_at
    if grep -q '"kind": 30311' "$tmp_file" 2>/dev/null; then
        local all_events
        all_events=$(awk '/^{$/,/^}$/ {print} /^}$/ {print "---SPLIT---"}' "$tmp_file" | \
            awk 'BEGIN{RS="---SPLIT---"} /"kind": 30311/ {print}' | \
            jq -s 'sort_by(.created_at) | reverse' 2>/dev/null)

        if [ -n "$d_filter" ]; then
            echo "$all_events" | jq --arg d "$d_filter" '[.[] | select(.tags[]? | select(.[0] == "d" and .[1] == $d))] | .[0]' 2>/dev/null
        else
            echo "$all_events" | jq '.[0]' 2>/dev/null
        fi
    else
        echo "null"
    fi

    rm -f "$tmp_file"
}

echo "========================================"
echo "External Backend E2E Integration Test"
echo "========================================"
echo ""
echo "Test Pubkey: $TEST_NPUB"
echo "API Port:    $API_PORT"
echo "Relay:       $NOSTR_RELAY_URL"
echo ""

# ── TEST 1: Prerequisites ──────────────────────────────────────────

echo "========================================"
echo "TEST 1: Prerequisites"
echo "========================================"

PREREQ_OK=true

if ! command -v node &> /dev/null; then
    echo "❌ node not found"
    PREREQ_OK=false
fi

if ! command -v jq &> /dev/null; then
    echo "❌ jq not found"
    PREREQ_OK=false
fi

if ! command -v ffmpeg &> /dev/null; then
    echo "❌ ffmpeg not found"
    PREREQ_OK=false
fi

if ! docker ps &> /dev/null; then
    echo "❌ Docker is not running"
    PREREQ_OK=false
fi

# Auto-detect container names
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
    echo ""
    fail_test "TEST 1"
    echo "Fix prerequisites above and retry."
    exit 1
fi

echo "✓ All prerequisites met"
pass_test "TEST 1: Prerequisites"
echo ""

# Decode npub to hex
TEST_PUBKEY_HEX=$(node "$SCRIPT_DIR/decode_npub.js" "$TEST_NPUB" 2>&1)
if [ $? -ne 0 ]; then
    echo "❌ Failed to decode npub"
    exit 1
fi
UPPER_PUBKEY=$(echo "$TEST_PUBKEY_HEX" | tr '[:lower:]' '[:upper:]')

# Ensure user exists in database
docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${TEST_PUBKEY_HEX}'), 0);" \
  2>/dev/null || true

echo "Test pubkey hex: $TEST_PUBKEY_HEX"
echo ""

# ── TEST 2: Check initial database state ───────────────────────────

echo "========================================"
echo "TEST 2: Check initial database state"
echo "========================================"

DB_EXT_ID_BEFORE=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_PUBKEY}';" -s -N 2>/dev/null)

if [ -z "$DB_EXT_ID_BEFORE" ] || [ "$DB_EXT_ID_BEFORE" == "NULL" ]; then
    echo "✓ User is NEW (no external_id in database)"
    USER_TYPE="NEW"
else
    echo "✓ User is EXISTING (has external_id: $DB_EXT_ID_BEFORE)"
    USER_TYPE="EXISTING"
fi

pass_test "TEST 2: Initial DB state"
echo ""

# ── TEST 3: API call with NIP-98 auth ─────────────────────────────

echo "========================================"
echo "TEST 3: API call creates/reuses Live Input"
echo "========================================"

API_URL="http://localhost:${API_PORT}/api/v1/account"
AUTH_TOKEN=$(make_auth_token "$TEST_NSEC" "$API_URL" "GET")

API_RESPONSE=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_TOKEN")

if ! echo "$API_RESPONSE" | jq -e '.endpoints' > /dev/null 2>&1; then
    fail_test "TEST 3: API call" "API call failed: $API_RESPONSE"
    exit 1
fi

echo "✓ API returned valid response with endpoints"
pass_test "TEST 3: API call"
echo ""

# ── TEST 4: Database contains valid external_id ────────────────────

echo "========================================"
echo "TEST 4: Database contains valid external_id"
echo "========================================"

DB_EXT_ID_AFTER=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_PUBKEY}';" -s -N 2>/dev/null)

if [ -z "$DB_EXT_ID_AFTER" ] || [ "$DB_EXT_ID_AFTER" == "NULL" ]; then
    fail_test "TEST 4: DB external_id" "No external_id in database after API call"
    exit 1
fi

# Validate UID format (32 hex chars)
if [[ ! $DB_EXT_ID_AFTER =~ ^[0-9a-f]{32}$ ]]; then
    fail_test "TEST 4: DB external_id" "Invalid external_id format: $DB_EXT_ID_AFTER"
    exit 1
fi

echo "✓ Database contains external_id: $DB_EXT_ID_AFTER"
pass_test "TEST 4: DB external_id"
echo ""

# ── TEST 5: RTMPS endpoint validation ─────────────────────────────

echo "========================================"
echo "TEST 5: RTMPS endpoint validation"
echo "========================================"

# Find the RTMPS endpoint (name starts with "RTMPS-")
RTMPS_ENDPOINT=$(echo "$API_RESPONSE" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-"))')

if [ -z "$RTMPS_ENDPOINT" ] || [ "$RTMPS_ENDPOINT" == "null" ]; then
    fail_test "TEST 5: RTMPS endpoint" "No RTMPS endpoint found in API response"
    echo "Endpoints: $(echo "$API_RESPONSE" | jq '.endpoints')"
    exit 1
fi

RTMP_URL=$(echo "$RTMPS_ENDPOINT" | jq -r '.url')
RTMP_KEY=$(echo "$RTMPS_ENDPOINT" | jq -r '.key')

if [[ ! $RTMP_URL =~ ^rtmps:// ]]; then
    fail_test "TEST 5: RTMPS endpoint" "Invalid RTMPS URL format: $RTMP_URL"
    exit 1
fi

echo "✓ RTMPS URL: $RTMP_URL"
echo "✓ RTMPS Key: ${RTMP_KEY:0:20}... (${#RTMP_KEY} chars)"
pass_test "TEST 5: RTMPS endpoint"
echo ""

# ── TEST 6: SRT endpoint validation ───────────────────────────────

echo "========================================"
echo "TEST 6: SRT endpoint validation"
echo "========================================"

SRT_ENDPOINT=$(echo "$API_RESPONSE" | jq -r '.endpoints[] | select(.name | startswith("SRT-"))')

if [ -z "$SRT_ENDPOINT" ] || [ "$SRT_ENDPOINT" == "null" ]; then
    echo "⚠️  No SRT endpoint in response (may not be enabled on this Cloudflare account)"
    echo "   Skipping SRT validation"
    SRT_AVAILABLE=false
    pass_test "TEST 6: SRT endpoint (skipped - not available)"
else
    SRT_URL=$(echo "$SRT_ENDPOINT" | jq -r '.url')
    SRT_KEY=$(echo "$SRT_ENDPOINT" | jq -r '.key')

    if [[ ! $SRT_URL =~ ^srt:// ]]; then
        fail_test "TEST 6: SRT endpoint" "Invalid SRT URL format: $SRT_URL"
    elif [[ ! $SRT_KEY =~ ^streamid=.*\&passphrase= ]]; then
        fail_test "TEST 6: SRT endpoint" "Invalid SRT key format: $SRT_KEY"
    else
        echo "✓ SRT URL: $SRT_URL"
        echo "✓ SRT Key: streamid=...&passphrase=... (${#SRT_KEY} chars)"
        SRT_AVAILABLE=true
        pass_test "TEST 6: SRT endpoint"
    fi
fi
echo ""

# ── TEST 7: Idempotency ───────────────────────────────────────────

echo "========================================"
echo "TEST 7: Second API call reuses same UID"
echo "========================================"

# Fresh auth token (timestamps differ)
AUTH_TOKEN_2=$(make_auth_token "$TEST_NSEC" "$API_URL" "GET")
API_RESPONSE_2=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_TOKEN_2")

RTMP_KEY_2=$(echo "$API_RESPONSE_2" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-")) | .key')

DB_EXT_ID_FINAL=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_PUBKEY}';" -s -N 2>/dev/null)

T7_OK=true
if [ "$DB_EXT_ID_AFTER" != "$DB_EXT_ID_FINAL" ]; then
    echo "❌ external_id changed! Before: $DB_EXT_ID_AFTER After: $DB_EXT_ID_FINAL"
    T7_OK=false
fi

if [ "$RTMP_KEY" != "$RTMP_KEY_2" ]; then
    echo "❌ Stream key changed between calls"
    T7_OK=false
fi

if [ "$T7_OK" == "true" ]; then
    echo "✓ Same external_id persisted: $DB_EXT_ID_FINAL"
    echo "✓ Same stream key returned"
    pass_test "TEST 7: Idempotency"
else
    fail_test "TEST 7: Idempotency" "Values changed between API calls"
fi
echo ""

# ── TEST 8: Custom keys ───────────────────────────────────────────

echo "========================================"
echo "TEST 8: Custom keys - create and list"
echo "========================================"

T8_OK=true

# 8a: Create a custom key
POST_URL="http://localhost:${API_PORT}/api/v1/keys"
POST_AUTH=$(make_auth_token "$TEST_NSEC" "$POST_URL" "POST")

CUSTOM_KEY_REQUEST='{
  "event": {
    "title": "E2E Test Stream",
    "summary": "External backend custom key test",
    "tags": ["test", "e2e"]
  }
}'

CREATE_RESPONSE=$(curl -s -X POST "$POST_URL" \
  -H "Authorization: Nostr $POST_AUTH" \
  -H "Content-Type: application/json" \
  -d "$CUSTOM_KEY_REQUEST")

CUSTOM_KEY=$(echo "$CREATE_RESPONSE" | jq -r '.key // empty')

if [ -z "$CUSTOM_KEY" ]; then
    fail_test "TEST 8: Custom keys" "Failed to create custom key: $CREATE_RESPONSE"
    # Custom keys are required, but continue with remaining tests
    CUSTOM_KEY_AVAILABLE=false
else
    echo "✓ Custom key created: ${CUSTOM_KEY:0:20}... (${#CUSTOM_KEY} chars)"
    CUSTOM_KEY_AVAILABLE=true

    # 8b: List keys and verify
    GET_KEYS_URL="http://localhost:${API_PORT}/api/v1/keys"
    GET_AUTH=$(make_auth_token "$TEST_NSEC" "$GET_KEYS_URL" "GET")

    KEYS_LIST=$(curl -s "$GET_KEYS_URL" -H "Authorization: Nostr $GET_AUTH")

    KEY_COUNT=$(echo "$KEYS_LIST" | jq 'length' 2>/dev/null)
    if [ -z "$KEY_COUNT" ] || [ "$KEY_COUNT" -lt 1 ]; then
        echo "❌ No keys returned from GET /api/v1/keys"
        T8_OK=false
    else
        echo "✓ GET /api/v1/keys returned $KEY_COUNT key(s)"
    fi

    # Find our key in the list and get associated stream_id
    CUSTOM_KEY_ENTRY=$(echo "$KEYS_LIST" | jq --arg key "$CUSTOM_KEY" '.[] | select(.key == $key)' 2>/dev/null)

    if [ -z "$CUSTOM_KEY_ENTRY" ] || [ "$CUSTOM_KEY_ENTRY" == "null" ]; then
        echo "❌ Custom key not found in key list"
        T8_OK=false
    else
        CUSTOM_KEY_STREAM_ID=$(echo "$CUSTOM_KEY_ENTRY" | jq -r '.stream_id')
        echo "✓ Custom key found in list, stream_id: $CUSTOM_KEY_STREAM_ID"
    fi

    if [ "$T8_OK" == "true" ]; then
        pass_test "TEST 8: Custom keys"
    else
        fail_test "TEST 8: Custom keys" "Key list validation failed"
    fi
fi
echo ""

# ── TEST 9: Stream via RTMPS ──────────────────────────────────────

echo "========================================"
echo "TEST 9: Stream via RTMPS to Cloudflare"
echo "========================================"

# Use the default account key for this test
RTMP_DEST="${RTMP_URL}${RTMP_KEY}"
FFMPEG_LOG=$(mktemp)

echo "Streaming to: ${RTMP_URL}(key)"

ffmpeg -re -t 30 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST" \
  </dev/null >>"$FFMPEG_LOG" 2>&1 &

FFMPEG_PID=$!

sleep 3
if ! ps -p $FFMPEG_PID > /dev/null 2>&1; then
    echo "❌ FFmpeg failed to start"
    cat "$FFMPEG_LOG"
    rm -f "$FFMPEG_LOG"
    fail_test "TEST 9: RTMPS stream" "FFmpeg died immediately"
    exit 1
fi

echo "✓ FFmpeg streaming (PID: $FFMPEG_PID)"
pass_test "TEST 9: RTMPS stream"
echo ""

# ── TEST 10: Webhooks trigger stream START ─────────────────────────

echo "========================================"
echo "TEST 10: Webhooks trigger stream START"
echo "========================================"

echo "Waiting 20 seconds for Cloudflare webhooks..."
sleep 20

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T10_CHECKS=0
T10_TOTAL=2

if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.connected"; then
    echo "✓ Webhook: live_input.connected"
    T10_CHECKS=$((T10_CHECKS + 1))
else
    echo "✗ Missing: live_input.connected webhook"
fi

if echo "$LOGS" | grep -q "Published stream event"; then
    echo "✓ Stream event published"
    T10_CHECKS=$((T10_CHECKS + 1))
else
    echo "✗ Missing: Published stream event"
fi

if [ $T10_CHECKS -eq $T10_TOTAL ]; then
    pass_test "TEST 10: Webhook START"
else
    fail_test "TEST 10: Webhook START" "$T10_CHECKS/$T10_TOTAL checks passed"
fi
echo ""

# ── TEST 11: Verify LIVE Nostr event ──────────────────────────────

echo "========================================"
echo "TEST 11: Verify LIVE Nostr event (kind 30311)"
echo "========================================"

SINCE_TIME=$(($(date +%s) - 600))
EVENT_JSON=$(query_latest_30311 "$SINCE_TIME")

T11_CHECKS=0
T11_TOTAL=4

if [ "$EVENT_JSON" == "null" ] || [ -z "$EVENT_JSON" ]; then
    fail_test "TEST 11: LIVE Nostr event" "No kind 30311 event found on relay"
else
    # Check status = live
    STATUS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
    if [ "$STATUS" == "live" ]; then
        echo "✓ Event status: live"
        T11_CHECKS=$((T11_CHECKS + 1))
    else
        echo "✗ Expected status 'live', got: '$STATUS'"
    fi

    # Check streaming tag
    STREAMING_URL=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "streaming")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$STREAMING_URL" ] && [ "$STREAMING_URL" != "null" ]; then
        echo "✓ Event has 'streaming' tag: ${STREAMING_URL:0:60}..."
        T11_CHECKS=$((T11_CHECKS + 1))
    else
        echo "✗ Missing 'streaming' tag"
    fi

    # Check starts tag
    STARTS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "starts")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$STARTS" ] && [ "$STARTS" != "null" ]; then
        echo "✓ Event has 'starts' timestamp: $STARTS"
        T11_CHECKS=$((T11_CHECKS + 1))
    else
        echo "✗ Missing 'starts' tag"
    fi

    # Check ends tag does NOT exist yet
    ENDS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "ends")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -z "$ENDS" ] || [ "$ENDS" == "null" ] || [ "$ENDS" == "" ]; then
        echo "✓ Event does NOT have 'ends' tag (correct for live)"
        T11_CHECKS=$((T11_CHECKS + 1))
    else
        echo "✗ Event has 'ends' tag but stream is still live"
    fi

    if [ $T11_CHECKS -eq $T11_TOTAL ]; then
        pass_test "TEST 11: LIVE Nostr event"
    else
        fail_test "TEST 11: LIVE Nostr event" "$T11_CHECKS/$T11_TOTAL checks passed"
    fi
fi
echo ""

# ── TEST 12: End stream ───────────────────────────────────────────

echo "========================================"
echo "TEST 12: End stream and verify END webhooks"
echo "========================================"

if ps -p $FFMPEG_PID > /dev/null 2>&1; then
    kill -9 $FFMPEG_PID 2>/dev/null || true
    pkill -9 -f "ffmpeg.*testsrc" 2>/dev/null || true
    echo "✓ Stream stopped"
else
    echo "⚠️  Stream already stopped"
fi
rm -f "$FFMPEG_LOG"

echo "Waiting 15 seconds for END webhooks..."
sleep 15

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T12_CHECKS=0
T12_TOTAL=2

if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.disconnected"; then
    echo "✓ Webhook: live_input.disconnected"
    T12_CHECKS=$((T12_CHECKS + 1))
else
    echo "✗ Missing: live_input.disconnected webhook"
fi

if echo "$LOGS" | grep -q "Stream ended"; then
    echo "✓ Stream ended"
    T12_CHECKS=$((T12_CHECKS + 1))
else
    echo "✗ Missing: Stream ended"
fi

if [ $T12_CHECKS -eq $T12_TOTAL ]; then
    pass_test "TEST 12: Stream END"
else
    fail_test "TEST 12: Stream END" "$T12_CHECKS/$T12_TOTAL checks passed"
fi
echo ""

# ── TEST 13: Verify ENDED Nostr event ─────────────────────────────

echo "========================================"
echo "TEST 13: Verify ENDED Nostr event (kind 30311)"
echo "========================================"

SINCE_TIME=$(($(date +%s) - 600))
EVENT_JSON=$(query_latest_30311 "$SINCE_TIME")

T13_CHECKS=0
T13_TOTAL=3

if [ "$EVENT_JSON" == "null" ] || [ -z "$EVENT_JSON" ]; then
    fail_test "TEST 13: ENDED Nostr event" "No kind 30311 event found on relay"
else
    # Check status = ended
    STATUS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
    if [ "$STATUS" == "ended" ]; then
        echo "✓ Event status: ended"
        T13_CHECKS=$((T13_CHECKS + 1))
    else
        echo "✗ Expected status 'ended', got: '$STATUS'"
    fi

    # Check ends tag now exists
    ENDS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "ends")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$ENDS" ] && [ "$ENDS" != "null" ] && [ "$ENDS" != "" ]; then
        echo "✓ Event has 'ends' timestamp: $ENDS"
        T13_CHECKS=$((T13_CHECKS + 1))
    else
        echo "✗ Missing 'ends' tag"
    fi

    # Check streaming tag is removed (replaced by recording or absent)
    STREAMING_URL=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "streaming")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -z "$STREAMING_URL" ] || [ "$STREAMING_URL" == "null" ] || [ "$STREAMING_URL" == "" ]; then
        echo "✓ 'streaming' tag removed (correct for ended)"
        T13_CHECKS=$((T13_CHECKS + 1))
    else
        echo "✗ 'streaming' tag still present: $STREAMING_URL"
    fi

    if [ $T13_CHECKS -eq $T13_TOTAL ]; then
        pass_test "TEST 13: ENDED Nostr event"
    else
        fail_test "TEST 13: ENDED Nostr event" "$T13_CHECKS/$T13_TOTAL checks passed"
    fi
fi
echo ""

# ── TEST 14: Custom key stream lifecycle ───────────────────────────

if [ "$CUSTOM_KEY_AVAILABLE" == "true" ]; then
    echo "========================================"
    echo "TEST 14: Stream with custom key"
    echo "========================================"

    # Get the RTMPS URL (same base URL, different key)
    CUSTOM_RTMP_DEST="${RTMP_URL}${CUSTOM_KEY}"
    FFMPEG_LOG_CK=$(mktemp)

    echo "Streaming with custom key to: ${RTMP_URL}(custom-key)"

    ffmpeg -re -t 30 \
      -f lavfi -i testsrc=size=1280x720:rate=30 \
      -f lavfi -i sine=frequency=800:sample_rate=44100 \
      -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
      -c:a aac -ar 44100 -b:a 128k \
      -f flv "$CUSTOM_RTMP_DEST" \
      </dev/null >>"$FFMPEG_LOG_CK" 2>&1 &

    CK_FFMPEG_PID=$!

    sleep 3
    if ! ps -p $CK_FFMPEG_PID > /dev/null 2>&1; then
        echo "❌ FFmpeg failed to start for custom key stream"
        cat "$FFMPEG_LOG_CK"
        rm -f "$FFMPEG_LOG_CK"
        fail_test "TEST 14: Custom key stream" "FFmpeg died"
    else
        echo "✓ FFmpeg streaming with custom key (PID: $CK_FFMPEG_PID)"

        echo "Waiting 20 seconds for webhooks..."
        sleep 20

        LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

        T14_CHECKS=0
        T14_TOTAL=2

        if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.connected"; then
            echo "✓ Webhook: live_input.connected for custom key"
            T14_CHECKS=$((T14_CHECKS + 1))
        else
            echo "✗ Missing: live_input.connected webhook for custom key"
        fi

        if echo "$LOGS" | grep -q "Published stream event"; then
            echo "✓ Stream event published for custom key"
            T14_CHECKS=$((T14_CHECKS + 1))
        else
            echo "✗ Missing: Published stream event for custom key"
        fi

        if [ $T14_CHECKS -eq $T14_TOTAL ]; then
            pass_test "TEST 14: Custom key stream"
        else
            fail_test "TEST 14: Custom key stream" "$T14_CHECKS/$T14_TOTAL checks passed"
        fi
        echo ""

        # ── TEST 15: Custom key Nostr event has metadata ───────────

        echo "========================================"
        echo "TEST 15: Custom key Nostr event metadata"
        echo "========================================"

        SINCE_TIME=$(($(date +%s) - 600))
        # Filter by custom key's stream_id (d tag) to avoid picking up the primary stream's event
        CK_EVENT_JSON=$(query_latest_30311 "$SINCE_TIME" "$CUSTOM_KEY_STREAM_ID")

        T15_CHECKS=0
        T15_TOTAL=4

        if [ "$CK_EVENT_JSON" == "null" ] || [ -z "$CK_EVENT_JSON" ]; then
            fail_test "TEST 15: Custom key Nostr metadata" "No kind 30311 event found for d=$CUSTOM_KEY_STREAM_ID"
        else
            # Check title
            TITLE=$(echo "$CK_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "title")? | .[1]?' 2>/dev/null | head -n 1)
            if [ "$TITLE" == "E2E Test Stream" ]; then
                echo "✓ Event has custom title: '$TITLE'"
                T15_CHECKS=$((T15_CHECKS + 1))
            else
                echo "✗ Expected title 'E2E Test Stream', got: '$TITLE'"
            fi

            # Check summary
            SUMMARY=$(echo "$CK_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "summary")? | .[1]?' 2>/dev/null | head -n 1)
            if [ "$SUMMARY" == "External backend custom key test" ]; then
                echo "✓ Event has custom summary: '$SUMMARY'"
                T15_CHECKS=$((T15_CHECKS + 1))
            else
                echo "✗ Expected summary 'External backend custom key test', got: '$SUMMARY'"
            fi

            # Check status (accept both 'live' and 'ended' - short test streams may complete before query)
            STATUS=$(echo "$CK_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
            if [ "$STATUS" == "live" ] || [ "$STATUS" == "ended" ]; then
                echo "✓ Event status: $STATUS"
                T15_CHECKS=$((T15_CHECKS + 1))
            else
                echo "✗ Expected status 'live' or 'ended', got: '$STATUS'"
            fi

            # Check 't' tags
            T_TAGS=$(echo "$CK_EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "t")? | .[1]?' 2>/dev/null)
            if echo "$T_TAGS" | grep -q "test"; then
                echo "✓ Event has tag: 'test'"
                T15_CHECKS=$((T15_CHECKS + 1))
            else
                echo "✗ Missing tag: 'test'"
            fi

            if [ $T15_CHECKS -eq $T15_TOTAL ]; then
                pass_test "TEST 15: Custom key Nostr metadata"
            else
                fail_test "TEST 15: Custom key Nostr metadata" "$T15_CHECKS/$T15_TOTAL checks passed"
            fi
        fi
        echo ""

        # Stop custom key stream
        echo "Stopping custom key stream..."
        if ps -p $CK_FFMPEG_PID > /dev/null 2>&1; then
            kill -9 $CK_FFMPEG_PID 2>/dev/null || true
        fi
        rm -f "$FFMPEG_LOG_CK"

        echo "Waiting 15 seconds for custom key END webhooks..."
        sleep 15

        # ── TEST 16: Custom key stream END Nostr event ─────────────

        echo "========================================"
        echo "TEST 16: Custom key stream ENDED Nostr event"
        echo "========================================"

        SINCE_TIME=$(($(date +%s) - 600))
        CK_END_EVENT=$(query_latest_30311 "$SINCE_TIME" "$CUSTOM_KEY_STREAM_ID")

        T16_CHECKS=0
        T16_TOTAL=2

        if [ "$CK_END_EVENT" == "null" ] || [ -z "$CK_END_EVENT" ]; then
            fail_test "TEST 16: Custom key ENDED event" "No kind 30311 event found for d=$CUSTOM_KEY_STREAM_ID"
        else
            STATUS=$(echo "$CK_END_EVENT" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
            if [ "$STATUS" == "ended" ]; then
                echo "✓ Event status: ended"
                T16_CHECKS=$((T16_CHECKS + 1))
            else
                echo "✗ Expected status 'ended', got: '$STATUS'"
            fi

            ENDS=$(echo "$CK_END_EVENT" | jq -r '.tags[]? | select(.[0] == "ends")? | .[1]?' 2>/dev/null | head -n 1)
            if [ -n "$ENDS" ] && [ "$ENDS" != "null" ] && [ "$ENDS" != "" ]; then
                echo "✓ Event has 'ends' timestamp: $ENDS"
                T16_CHECKS=$((T16_CHECKS + 1))
            else
                echo "✗ Missing 'ends' tag"
            fi

            if [ $T16_CHECKS -eq $T16_TOTAL ]; then
                pass_test "TEST 16: Custom key ENDED event"
            else
                fail_test "TEST 16: Custom key ENDED event" "$T16_CHECKS/$T16_TOTAL checks passed"
            fi
        fi
        echo ""
    fi
else
    echo "========================================"
    echo "TEST 14-16: Skipped (custom key creation failed)"
    echo "========================================"
    echo ""
fi

# ── Cleanup ────────────────────────────────────────────────────────

# Kill any remaining ffmpeg processes from this test
pkill -9 -f "ffmpeg.*testsrc" 2>/dev/null || true

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
echo "Configuration used:"
echo "  API Port:           $API_PORT"
echo "  External container: $EXTERNAL_CONTAINER"
echo "  DB container:       $DB_CONTAINER"
echo "  Relay:              $NOSTR_RELAY_URL"
echo ""
echo "To review logs:"
echo "  docker logs --tail 300 $EXTERNAL_CONTAINER"

exit $TESTS_FAILED
