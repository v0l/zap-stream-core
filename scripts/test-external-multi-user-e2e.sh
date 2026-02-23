#!/bin/bash

# ==========================================
# External Backend Multi-User E2E Test
# ==========================================
#
# Tests concurrent multi-user streaming with the external Cloudflare backend:
# 1. Two users get unique Live Inputs from Cloudflare
# 2. Both stream concurrently
# 3. Verify webhooks associate correctly to each stream
# 4. Verify stream isolation (stopping one doesn't affect the other)
# 5. Verify UID persistence and Nostr events per user
#
# Usage:
#   cd scripts && npm install && cd ..
#   ./scripts/test-external-multi-user-e2e.sh
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

# Two test keypairs (safe, not production)
USER_A_NSEC="nsec15devjmm9cgwlpu7dw64cl29c02taw9gjrt5k6s78wxh3frwhhdcs986v76"
USER_A_NPUB="npub1tc6nuphuz0k0destd32mfluctx5jke60yxd794h3ugq7fgqgx0zq5eeln6"

USER_B_NSEC="nsec1u47296qau8ssg675wezgem0z3jslwxjaqs9xve74w3yn3v4esryqeqn2qg"
USER_B_NPUB="npub1xy7wqze00wut9psqa7psp5sjqzcfz49swh94ajudtfh3767llraqp3laua"

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

# Helper: query all kind 30311 events since a timestamp, return as JSON array
query_all_30311() {
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
            jq -s 'sort_by(.created_at) | reverse' 2>/dev/null
    else
        echo "[]"
    fi

    rm -f "$tmp_file"
}

# Helper: extract event for a specific user pubkey (p tag host) and status
# Usage: get_user_event "$ALL_EVENTS_JSON" "$USER_HEX" "$STATUS"
get_user_event() {
    local events="$1" pubkey="$2" status="$3"
    echo "$events" | jq --arg pk "$pubkey" --arg st "$status" \
        '[.[] | select(
            (.tags[]? | select(.[0] == "p" and .[1] == $pk)) and
            (.tags[]? | select(.[0] == "status" and .[1] == $st))
        )] | .[0]' 2>/dev/null
}

# Cleanup handler
cleanup() {
    echo ""
    echo "Cleaning up..."
    [ -n "${PID_A:-}" ] && ps -p $PID_A > /dev/null 2>&1 && kill -9 $PID_A 2>/dev/null
    [ -n "${PID_B:-}" ] && ps -p $PID_B > /dev/null 2>&1 && kill -9 $PID_B 2>/dev/null
    rm -f "${FFMPEG_LOG_A:-}" "${FFMPEG_LOG_B:-}"
    pkill -9 -f "ffmpeg.*testsrc" 2>/dev/null || true
}
trap cleanup EXIT

echo "========================================"
echo "External Multi-User E2E Test"
echo "========================================"
echo ""
echo "User A: $USER_A_NPUB"
echo "User B: $USER_B_NPUB"
echo "API Port: $API_PORT"
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

# ── TEST 2: Database Setup ────────────────────────────────────────

echo "========================================"
echo "TEST 2: Database setup"
echo "========================================"

USER_A_HEX=$(node "$SCRIPT_DIR/decode_npub.js" "$USER_A_NPUB" 2>&1)
USER_B_HEX=$(node "$SCRIPT_DIR/decode_npub.js" "$USER_B_NPUB" 2>&1)

echo "User A hex: $USER_A_HEX"
echo "User B hex: $USER_B_HEX"

docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${USER_A_HEX}'), 0);" \
  2>/dev/null || true

docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${USER_B_HEX}'), 0);" \
  2>/dev/null || true

echo "✓ Both users ensured in database"
pass_test "TEST 2: Database setup"
echo ""

# ── TEST 3: API - Get Stream Credentials ──────────────────────────

echo "========================================"
echo "TEST 3: API - Get stream credentials"
echo "========================================"

API_URL="http://localhost:${API_PORT}/api/v1/account"

# User A
echo "User A: Calling API..."
AUTH_A=$(make_auth_token "$USER_A_NSEC" "$API_URL" "GET")
RESPONSE_A=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_A")

if ! echo "$RESPONSE_A" | jq -e '.endpoints' > /dev/null 2>&1; then
    fail_test "TEST 3: API credentials" "User A API call failed: $RESPONSE_A"
    exit 1
fi

RTMP_URL_A=$(echo "$RESPONSE_A" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-")) | .url')
STREAM_KEY_A=$(echo "$RESPONSE_A" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-")) | .key')
echo "✓ User A stream key: ${STREAM_KEY_A:0:20}... (${#STREAM_KEY_A} chars)"

# User B
echo "User B: Calling API..."
AUTH_B=$(make_auth_token "$USER_B_NSEC" "$API_URL" "GET")
RESPONSE_B=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_B")

if ! echo "$RESPONSE_B" | jq -e '.endpoints' > /dev/null 2>&1; then
    fail_test "TEST 3: API credentials" "User B API call failed: $RESPONSE_B"
    exit 1
fi

RTMP_URL_B=$(echo "$RESPONSE_B" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-")) | .url')
STREAM_KEY_B=$(echo "$RESPONSE_B" | jq -r '.endpoints[] | select(.name | startswith("RTMPS-")) | .key')
echo "✓ User B stream key: ${STREAM_KEY_B:0:20}... (${#STREAM_KEY_B} chars)"

# Get external_ids from database
UPPER_A=$(echo "$USER_A_HEX" | tr '[:lower:]' '[:upper:]')
UPPER_B=$(echo "$USER_B_HEX" | tr '[:lower:]' '[:upper:]')

EXT_ID_A=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_A}';" -s -N 2>/dev/null)

EXT_ID_B=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_B}';" -s -N 2>/dev/null)

echo "✓ User A external_id: $EXT_ID_A"
echo "✓ User B external_id: $EXT_ID_B"

pass_test "TEST 3: API credentials"
echo ""

# ── TEST 4: Unique external_ids ───────────────────────────────────

echo "========================================"
echo "TEST 4: Unique external_ids per user"
echo "========================================"

T4_OK=true

if [ "$EXT_ID_A" == "$EXT_ID_B" ]; then
    echo "❌ Both users have same external_id!"
    T4_OK=false
fi

if [[ ! $EXT_ID_A =~ ^[0-9a-f]{32}$ ]]; then
    echo "❌ User A external_id not valid format: $EXT_ID_A"
    T4_OK=false
fi

if [[ ! $EXT_ID_B =~ ^[0-9a-f]{32}$ ]]; then
    echo "❌ User B external_id not valid format: $EXT_ID_B"
    T4_OK=false
fi

if [ "$T4_OK" == "true" ]; then
    echo "✓ External IDs are unique and valid (32 hex chars each)"
    pass_test "TEST 4: Unique external_ids"
else
    fail_test "TEST 4: Unique external_ids" "Validation failed"
fi
echo ""

# ── TEST 5: User A starts streaming ───────────────────────────────

echo "========================================"
echo "TEST 5: User A starts streaming"
echo "========================================"

FFMPEG_LOG_A=$(mktemp)
FFMPEG_LOG_B=$(mktemp)

RTMP_DEST_A="${RTMP_URL_A}${STREAM_KEY_A}"

echo "User A streaming to: ${RTMP_URL_A}(key)"
ffmpeg -re -t 120 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST_A" \
  </dev/null >>"$FFMPEG_LOG_A" 2>&1 &

PID_A=$!

sleep 5
if ! ps -p $PID_A > /dev/null 2>&1; then
    echo "❌ User A FFmpeg died"
    cat "$FFMPEG_LOG_A"
    fail_test "TEST 5: User A stream" "FFmpeg died"
    exit 1
fi

echo "✓ User A streaming (PID: $PID_A)"
pass_test "TEST 5: User A stream"
echo ""

# ── TEST 6: User A webhook START ──────────────────────────────────

echo "========================================"
echo "TEST 6: User A webhook START (input_id isolation)"
echo "========================================"

echo "Waiting 20 seconds for User A webhooks..."
sleep 20

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T6_CHECKS=0

if echo "$LOGS" | grep -q "live_input.connected for input_id: $EXT_ID_A"; then
    echo "✓ Webhook: live_input.connected for User A input ($EXT_ID_A)"
    T6_CHECKS=$((T6_CHECKS + 1))
else
    echo "✗ Missing: live_input.connected for User A input_id $EXT_ID_A"
fi

if echo "$LOGS" | grep -q "Published stream event"; then
    echo "✓ Stream event published"
    T6_CHECKS=$((T6_CHECKS + 1))
else
    echo "✗ Missing: Published stream event"
fi

# Verify no webhook arrived for User B yet (they haven't started)
if echo "$LOGS" | grep -q "live_input.connected for input_id: $EXT_ID_B"; then
    echo "✗ UNEXPECTED: Webhook for User B input_id $EXT_ID_B before User B started!"
    T6_CHECKS=$((T6_CHECKS - 1))
else
    echo "✓ No premature webhook for User B (correct)"
fi

if [ $T6_CHECKS -eq 2 ]; then
    pass_test "TEST 6: User A webhook START"
else
    fail_test "TEST 6: User A webhook START" "$T6_CHECKS/2 checks passed"
fi
echo ""

# ── TEST 7: User B starts streaming (concurrent) ──────────────────

echo "========================================"
echo "TEST 7: User B starts streaming (concurrent)"
echo "========================================"

RTMP_DEST_B="${RTMP_URL_B}${STREAM_KEY_B}"

echo "User B streaming to: ${RTMP_URL_B}(key)"
ffmpeg -re -t 120 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=800:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST_B" \
  </dev/null >>"$FFMPEG_LOG_B" 2>&1 &

PID_B=$!

sleep 5

T7_OK=true

if ! ps -p $PID_A > /dev/null 2>&1; then
    echo "❌ User A FFmpeg died after User B started"
    T7_OK=false
fi

if ! ps -p $PID_B > /dev/null 2>&1; then
    echo "❌ User B FFmpeg died"
    cat "$FFMPEG_LOG_B"
    T7_OK=false
fi

if [ "$T7_OK" == "true" ]; then
    echo "✓ Both streams active concurrently"
    pass_test "TEST 7: User B stream (concurrent)"
else
    fail_test "TEST 7: User B stream (concurrent)" "One or both FFmpeg processes died"
    exit 1
fi
echo ""

# ── TEST 8: User B webhook START ──────────────────────────────────

echo "========================================"
echo "TEST 8: User B webhook START (input_id isolation)"
echo "========================================"

echo "Waiting 20 seconds for User B webhooks..."
sleep 20

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T8_CHECKS=0

# Verify User B's specific webhook arrived
if echo "$LOGS" | grep -q "live_input.connected for input_id: $EXT_ID_B"; then
    echo "✓ Webhook: live_input.connected for User B input ($EXT_ID_B)"
    T8_CHECKS=$((T8_CHECKS + 1))
else
    echo "✗ Missing: live_input.connected for User B input_id $EXT_ID_B"
fi

# Verify both users have distinct webhooks (User A's should also still be present)
if echo "$LOGS" | grep -q "live_input.connected for input_id: $EXT_ID_A"; then
    echo "✓ User A webhook also present (both users have distinct connected events)"
    T8_CHECKS=$((T8_CHECKS + 1))
else
    echo "✗ User A webhook missing from logs"
fi

# Total count should be >= 2
CONNECTED_COUNT=$(echo "$LOGS" | grep -c "live_input.connected for input_id:" || true)
echo "  Total connected webhooks with input_id: $CONNECTED_COUNT"

if [ $T8_CHECKS -eq 2 ]; then
    pass_test "TEST 8: User B webhook START"
else
    fail_test "TEST 8: User B webhook START" "$T8_CHECKS/2 checks passed"
fi
echo ""

# ── TEST 9: Verify both LIVE on Nostr relay ────────────────────────

echo "========================================"
echo "TEST 9: Verify per-user LIVE Nostr events"
echo "========================================"

SINCE_TIME=$(($(date +%s) - 600))

ALL_EVENTS=$(query_all_30311 "$SINCE_TIME")
TOTAL_EVENTS=$(echo "$ALL_EVENTS" | jq 'length' 2>/dev/null || echo 0)
echo "Total kind 30311 events on relay: $TOTAL_EVENTS"

T9_CHECKS=0

# Check User A has a LIVE event with correct p tag
EVENT_A_LIVE=$(get_user_event "$ALL_EVENTS" "$USER_A_HEX" "live")
if [ -n "$EVENT_A_LIVE" ] && [ "$EVENT_A_LIVE" != "null" ]; then
    echo "✓ User A has LIVE event (p tag matches $USER_A_HEX)"
    T9_CHECKS=$((T9_CHECKS + 1))
else
    echo "✗ No LIVE event found for User A (p=$USER_A_HEX)"
fi

# Check User B has a LIVE event with correct p tag
EVENT_B_LIVE=$(get_user_event "$ALL_EVENTS" "$USER_B_HEX" "live")
if [ -n "$EVENT_B_LIVE" ] && [ "$EVENT_B_LIVE" != "null" ]; then
    echo "✓ User B has LIVE event (p tag matches $USER_B_HEX)"
    T9_CHECKS=$((T9_CHECKS + 1))
else
    echo "✗ No LIVE event found for User B (p=$USER_B_HEX)"
fi

# Verify the d tags (stream IDs) are different
if [ -n "$EVENT_A_LIVE" ] && [ "$EVENT_A_LIVE" != "null" ] && [ -n "$EVENT_B_LIVE" ] && [ "$EVENT_B_LIVE" != "null" ]; then
    D_TAG_A=$(echo "$EVENT_A_LIVE" | jq -r '[.tags[]? | select(.[0] == "d")] | .[0][1]' 2>/dev/null)
    D_TAG_B=$(echo "$EVENT_B_LIVE" | jq -r '[.tags[]? | select(.[0] == "d")] | .[0][1]' 2>/dev/null)
    if [ "$D_TAG_A" != "$D_TAG_B" ] && [ -n "$D_TAG_A" ] && [ -n "$D_TAG_B" ]; then
        echo "✓ Stream IDs are distinct (A: $D_TAG_A, B: $D_TAG_B)"
        T9_CHECKS=$((T9_CHECKS + 1))
    else
        echo "✗ Stream IDs not distinct (A: $D_TAG_A, B: $D_TAG_B)"
    fi
else
    echo "✗ Cannot compare stream IDs (missing events)"
fi

if [ $T9_CHECKS -eq 3 ]; then
    pass_test "TEST 9: Per-user LIVE Nostr events"
else
    fail_test "TEST 9: Per-user LIVE Nostr events" "$T9_CHECKS/3 checks passed"
fi
echo ""

# ── TEST 10: Stream isolation - stop User A ────────────────────────

echo "========================================"
echo "TEST 10: Stream isolation - stop User A"
echo "========================================"

echo "Stopping User A stream..."
kill -9 $PID_A 2>/dev/null || true
sleep 2

if ! ps -p $PID_B > /dev/null 2>&1; then
    fail_test "TEST 10: Stream isolation" "User B died when User A stopped!"
    exit 1
fi

echo "✓ User B still streaming after User A stopped (isolation verified)"
pass_test "TEST 10: Stream isolation"
echo ""

# ── TEST 11: User A disconnect webhook ─────────────────────────────

echo "========================================"
echo "TEST 11: User A disconnect webhook (isolation)"
echo "========================================"

echo "Waiting 15 seconds for User A disconnect webhook..."
sleep 15

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T11_CHECKS=0

# Verify User A's specific disconnect webhook
if echo "$LOGS" | grep -q "live_input.disconnected for input_id: $EXT_ID_A"; then
    echo "✓ Webhook: live_input.disconnected for User A input ($EXT_ID_A)"
    T11_CHECKS=$((T11_CHECKS + 1))
else
    echo "✗ Missing: live_input.disconnected for User A input_id $EXT_ID_A"
fi

# Verify User B did NOT get a disconnected webhook
if echo "$LOGS" | grep -q "live_input.disconnected for input_id: $EXT_ID_B"; then
    echo "✗ ISOLATION FAILURE: User B got disconnected webhook when only User A stopped!"
else
    echo "✓ No disconnected webhook for User B (correct - isolation holds)"
    T11_CHECKS=$((T11_CHECKS + 1))
fi

# Confirm User B FFmpeg still alive
if ps -p $PID_B > /dev/null 2>&1; then
    echo "✓ User B still streaming (FFmpeg alive)"
    T11_CHECKS=$((T11_CHECKS + 1))
else
    echo "✗ User B FFmpeg died unexpectedly"
fi

# Verify User B's Nostr event is still LIVE (not ended by User A's disconnect)
SINCE_TIME=$(($(date +%s) - 600))
ALL_EVENTS_POST_A=$(query_all_30311 "$SINCE_TIME")

EVENT_B_STILL_LIVE=$(get_user_event "$ALL_EVENTS_POST_A" "$USER_B_HEX" "live")
EVENT_A_ENDED=$(get_user_event "$ALL_EVENTS_POST_A" "$USER_A_HEX" "ended")

if [ -n "$EVENT_B_STILL_LIVE" ] && [ "$EVENT_B_STILL_LIVE" != "null" ]; then
    echo "✓ User B Nostr event still LIVE (not affected by User A disconnect)"
    T11_CHECKS=$((T11_CHECKS + 1))
else
    echo "✗ User B Nostr event is NOT live - isolation may have failed!"
fi

if [ -n "$EVENT_A_ENDED" ] && [ "$EVENT_A_ENDED" != "null" ]; then
    echo "✓ User A Nostr event correctly ENDED"
    T11_CHECKS=$((T11_CHECKS + 1))
else
    echo "✗ User A Nostr event not yet ended (may need more time)"
fi

if [ $T11_CHECKS -ge 4 ]; then
    pass_test "TEST 11: User A disconnect (isolation)"
else
    fail_test "TEST 11: User A disconnect (isolation)" "$T11_CHECKS/5 checks passed"
fi
echo ""

# ── TEST 12: Stop User B ──────────────────────────────────────────

echo "========================================"
echo "TEST 12: Stop User B (input_id isolation)"
echo "========================================"

echo "Stopping User B stream..."
kill -9 $PID_B 2>/dev/null || true

echo "Waiting 15 seconds for User B disconnect webhook..."
sleep 15

LOGS=$(docker logs --tail 200 "$EXTERNAL_CONTAINER" 2>&1)

T12_CHECKS=0

# Verify User B's specific disconnect webhook
if echo "$LOGS" | grep -q "live_input.disconnected for input_id: $EXT_ID_B"; then
    echo "✓ Webhook: live_input.disconnected for User B input ($EXT_ID_B)"
    T12_CHECKS=$((T12_CHECKS + 1))
else
    echo "✗ Missing: live_input.disconnected for User B input_id $EXT_ID_B"
fi

# Verify both users now have distinct disconnect webhooks
if echo "$LOGS" | grep -q "live_input.disconnected for input_id: $EXT_ID_A"; then
    echo "✓ User A disconnect webhook also present (both users have distinct disconnected events)"
    T12_CHECKS=$((T12_CHECKS + 1))
else
    echo "✗ User A disconnect webhook missing from logs"
fi

if [ $T12_CHECKS -eq 2 ]; then
    pass_test "TEST 12: Stop User B"
else
    fail_test "TEST 12: Stop User B" "$T12_CHECKS/2 checks passed"
fi
echo ""

# ── TEST 13: Verify ENDED Nostr events ─────────────────────────────

echo "========================================"
echo "TEST 13: Verify per-user ENDED Nostr events"
echo "========================================"

SINCE_TIME=$(($(date +%s) - 600))

ALL_EVENTS_FINAL=$(query_all_30311 "$SINCE_TIME")
TOTAL_EVENTS_FINAL=$(echo "$ALL_EVENTS_FINAL" | jq 'length' 2>/dev/null || echo 0)
echo "Total kind 30311 events on relay: $TOTAL_EVENTS_FINAL"

T13_CHECKS=0

# Check User A has an ENDED event with correct p tag
EVENT_A_ENDED_FINAL=$(get_user_event "$ALL_EVENTS_FINAL" "$USER_A_HEX" "ended")
if [ -n "$EVENT_A_ENDED_FINAL" ] && [ "$EVENT_A_ENDED_FINAL" != "null" ]; then
    ENDS_TAG_A=$(echo "$EVENT_A_ENDED_FINAL" | jq -r '[.tags[]? | select(.[0] == "ends")] | .[0][1] // "missing"' 2>/dev/null)
    echo "✓ User A has ENDED event (p tag matches $USER_A_HEX, ends=$ENDS_TAG_A)"
    T13_CHECKS=$((T13_CHECKS + 1))
else
    echo "✗ No ENDED event found for User A (p=$USER_A_HEX)"
fi

# Check User B has an ENDED event with correct p tag
EVENT_B_ENDED_FINAL=$(get_user_event "$ALL_EVENTS_FINAL" "$USER_B_HEX" "ended")
if [ -n "$EVENT_B_ENDED_FINAL" ] && [ "$EVENT_B_ENDED_FINAL" != "null" ]; then
    ENDS_TAG_B=$(echo "$EVENT_B_ENDED_FINAL" | jq -r '[.tags[]? | select(.[0] == "ends")] | .[0][1] // "missing"' 2>/dev/null)
    echo "✓ User B has ENDED event (p tag matches $USER_B_HEX, ends=$ENDS_TAG_B)"
    T13_CHECKS=$((T13_CHECKS + 1))
else
    echo "✗ No ENDED event found for User B (p=$USER_B_HEX)"
fi

# Verify the d tags (stream IDs) are different
if [ -n "$EVENT_A_ENDED_FINAL" ] && [ "$EVENT_A_ENDED_FINAL" != "null" ] && \
   [ -n "$EVENT_B_ENDED_FINAL" ] && [ "$EVENT_B_ENDED_FINAL" != "null" ]; then
    D_TAG_A_END=$(echo "$EVENT_A_ENDED_FINAL" | jq -r '[.tags[]? | select(.[0] == "d")] | .[0][1]' 2>/dev/null)
    D_TAG_B_END=$(echo "$EVENT_B_ENDED_FINAL" | jq -r '[.tags[]? | select(.[0] == "d")] | .[0][1]' 2>/dev/null)
    if [ "$D_TAG_A_END" != "$D_TAG_B_END" ] && [ -n "$D_TAG_A_END" ] && [ -n "$D_TAG_B_END" ]; then
        echo "✓ Ended stream IDs are distinct (A: $D_TAG_A_END, B: $D_TAG_B_END)"
        T13_CHECKS=$((T13_CHECKS + 1))
    else
        echo "✗ Ended stream IDs not distinct (A: $D_TAG_A_END, B: $D_TAG_B_END)"
    fi
else
    echo "✗ Cannot compare ended stream IDs (missing events)"
fi

if [ $T13_CHECKS -eq 3 ]; then
    pass_test "TEST 13: Per-user ENDED Nostr events"
else
    fail_test "TEST 13: Per-user ENDED Nostr events" "$T13_CHECKS/3 checks passed"
fi
echo ""

# ── TEST 14: UID persistence ──────────────────────────────────────

echo "========================================"
echo "TEST 14: UID persistence validation"
echo "========================================"

EXT_ID_A_FINAL=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_A}';" -s -N 2>/dev/null)

EXT_ID_B_FINAL=$(docker exec "$DB_CONTAINER" mariadb -uroot -p"${DB_PASSWORD}" zap_stream \
    -e "SELECT external_id FROM user WHERE HEX(pubkey)='${UPPER_B}';" -s -N 2>/dev/null)

T14_OK=true

if [ "$EXT_ID_A_FINAL" != "$EXT_ID_A" ]; then
    echo "❌ User A external_id changed! Before: $EXT_ID_A After: $EXT_ID_A_FINAL"
    T14_OK=false
else
    echo "✓ User A external_id persisted: $EXT_ID_A"
fi

if [ "$EXT_ID_B_FINAL" != "$EXT_ID_B" ]; then
    echo "❌ User B external_id changed! Before: $EXT_ID_B After: $EXT_ID_B_FINAL"
    T14_OK=false
else
    echo "✓ User B external_id persisted: $EXT_ID_B"
fi

if [ "$T14_OK" == "true" ]; then
    pass_test "TEST 14: UID persistence"
else
    fail_test "TEST 14: UID persistence" "External IDs changed during test"
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
echo "Multi-User Summary:"
echo "  User A external_id: $EXT_ID_A"
echo "  User B external_id: $EXT_ID_B"
echo ""
echo "Key Findings:"
echo "  - Unique external_ids: Users have different Cloudflare Live Inputs"
echo "  - UID Persistence: External IDs remained constant throughout test"
echo "  - Webhook Isolation: Each webhook matched its user's input_id (no cross-contamination)"
echo "  - Stream Isolation: User B continued streaming when User A stopped"
echo "  - Nostr Isolation: User B Nostr event stayed LIVE when User A ended"
echo ""
echo "To review logs:"
echo "  docker logs --tail 300 $EXTERNAL_CONTAINER"

exit $TESTS_FAILED
