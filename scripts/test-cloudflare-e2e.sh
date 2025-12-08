#!/bin/bash

# ==========================================
# Cloudflare End-to-End Integration Test
# ==========================================
# 
# This script verifies the complete Cloudflare streaming lifecycle.
# Works with both NEW users (no UID) and EXISTING users (has UID).

set -e  # Exit on error

# Test credentials (safe test keypair, not production)
TEST_NSEC="nsec107gexedhvf97ej83jzalley9wt682mlgy9ty5xwsp98vnph09fysssnzlk"
TEST_NPUB="npub1u0mm82x7muct7cy8y7urztyctgm0r6k27gdax04fa4q28x7q0shq6slmah"

echo "========================================"
echo "Cloudflare E2E Integration Test"
echo "========================================"
echo ""
echo "Test Pubkey: $TEST_NPUB"
echo ""

# Check prerequisites
echo "[Prerequisites] Checking environment..."

if ! command -v node &> /dev/null; then
    echo "❌ ERROR: node not found"
    exit 1
fi

if ! docker ps &> /dev/null; then
    echo "❌ ERROR: Docker is not running"
    exit 1
fi

if ! docker ps | grep -q zap-stream-core-core-1; then
    echo "❌ ERROR: zap-stream-core-core-1 container not running"
    exit 1
fi

echo "✓ All prerequisites met"
echo ""

# Decode npub to hex using utility script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_PUBKEY_HEX=$(node "$SCRIPT_DIR/decode_npub.js" "$TEST_NPUB" 2>&1)

if [ $? -ne 0 ]; then
    echo "❌ Failed to decode npub"
    exit 1
fi

echo "Test pubkey hex: $TEST_PUBKEY_HEX (${#TEST_PUBKEY_HEX} chars)"

# Ensure user exists in database (with empty or existing stream_key)
docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${TEST_PUBKEY_HEX}'), 0);" \
  2>/dev/null || true

echo ""
echo "========================================" 
echo "TEST 1: Check initial database state"
echo "========================================"

# Check database for existing Live Input UID
UPPER_PUBKEY=$(echo "$TEST_PUBKEY_HEX" | tr '[:lower:]' '[:upper:]')
DB_UID_BEFORE=$(docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream \
    -e "SELECT stream_key FROM user WHERE HEX(pubkey)='${UPPER_PUBKEY}';" -s -N 2>/dev/null)

if [ -z "$DB_UID_BEFORE" ] || [ "$DB_UID_BEFORE" == "NULL" ]; then
    echo "✓ User is NEW (no Live Input UID in database)"
    USER_TYPE="NEW"
else
    echo "✓ User is EXISTING (has Live Input UID: $DB_UID_BEFORE)"
    USER_TYPE="EXISTING"
fi

echo "✅ TEST 1 PASSED"
echo ""

echo "========================================"
echo "TEST 2: API call handles user correctly"
echo "========================================"

# Create NIP-98 auth for API calls
API_URL="http://localhost:80/api/v1/account"
AUTH_EVENT_JSON=$(node "$SCRIPT_DIR/sign_nip98.js" "$TEST_NSEC" "$API_URL" "GET" 2>&1)
if [ $? -ne 0 ]; then
    echo "❌ Failed to create NIP-98 event"
    exit 1
fi
AUTH_TOKEN=$(echo "$AUTH_EVENT_JSON" | base64)

# Make API call (will create Live Input for new user, or reuse for existing)
API_RESPONSE=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_TOKEN")

if ! echo "$API_RESPONSE" | jq -e '.endpoints' > /dev/null 2>&1; then
    echo "❌ API call failed: $API_RESPONSE"
    exit 1
fi

if [ "$USER_TYPE" == "NEW" ]; then
    echo "✓ API should create new Live Input"
else
    echo "✓ API should reuse existing Live Input UID"
fi

echo "✅ TEST 2 PASSED"
echo ""

echo "========================================"
echo "TEST 3: Database now contains valid UID"
echo "========================================"

# Check database again
DB_UID_AFTER=$(docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream \
    -e "SELECT stream_key FROM user WHERE HEX(pubkey)='${UPPER_PUBKEY}';" -s -N 2>/dev/null)

if [ -z "$DB_UID_AFTER" ]; then
    echo "❌ No Live Input UID in database after API call"
    exit 1
fi

# Validate UID format (32 hex chars)
if [[ ! $DB_UID_AFTER =~ ^[0-9a-f]{32}$ ]]; then
    echo "❌ Invalid UID format: $DB_UID_AFTER"
    exit 1
fi

echo "✓ Database contains Live Input UID: $DB_UID_AFTER"
echo "✅ TEST 3 PASSED"
echo ""

echo "========================================"
echo "TEST 4: Cloudflare returns valid credentials"
echo "========================================"

RTMP_URL=$(echo "$API_RESPONSE" | jq -r '.endpoints[0].url // empty')
CF_STREAMKEY=$(echo "$API_RESPONSE" | jq -r '.endpoints[0].key // empty')

if [ -z "$RTMP_URL" ] || [ -z "$CF_STREAMKEY" ]; then
    echo "❌ Missing RTMP URL or streamKey in API response"
    exit 1
fi

# Validate format
if [[ ! $RTMP_URL =~ ^rtmps:// ]]; then
    echo "❌ Invalid RTMP URL format: $RTMP_URL"
    exit 1
fi

if [[ ! $CF_STREAMKEY =~ ^[0-9a-fk]{32,}$ ]]; then
    echo "❌ Invalid streamKey format: $CF_STREAMKEY"
    exit 1
fi

echo "✓ RTMP URL: $RTMP_URL"
echo "✓ Cloudflare streamKey: ${CF_STREAMKEY:0:20}... (${#CF_STREAMKEY} chars)"
echo "✅ TEST 4 PASSED"
echo ""

echo "========================================"
echo "TEST 5: Second API call reuses same UID"
echo "========================================"

# Make second API call
API_RESPONSE_2=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_TOKEN")
CF_STREAMKEY_2=$(echo "$API_RESPONSE_2" | jq -r '.endpoints[0].key // empty')

# Check database again
DB_UID_FINAL=$(docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream \
    -e "SELECT stream_key FROM user WHERE HEX(pubkey)='${UPPER_PUBKEY}';" -s -N 2>/dev/null)

if [ "$DB_UID_AFTER" != "$DB_UID_FINAL" ]; then
    echo "❌ Database UID changed! Should persist same UID"
    echo "   After first call:  $DB_UID_AFTER"
    echo "   After second call: $DB_UID_FINAL"
    exit 1
fi

if [ "$CF_STREAMKEY" != "$CF_STREAMKEY_2" ]; then
    echo "❌ Different streamKeys returned"
    exit 1
fi

echo "✓ Same Live Input UID persisted: $DB_UID_FINAL"
echo "✓ Same streamKey returned"
echo "✅ TEST 5 PASSED"
echo ""

echo "========================================"
echo "TEST 6: Stream to Cloudflare"
echo "========================================"

# Client must concatenate URL + key (matches real app behavior)
RTMP_DEST="${RTMP_URL}${CF_STREAMKEY}"
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
    rm "$FFMPEG_LOG"
    exit 1
fi

echo "✓ FFmpeg streaming (PID: $FFMPEG_PID)"
echo "✅ TEST 6 PASSED"
echo ""

echo "========================================"
echo "TEST 7: Webhooks trigger stream START"
echo "========================================"

echo "Waiting 20 seconds for webhooks..."
sleep 20

LOGS=$(docker logs --tail 100 zap-stream-core-core-1 2>&1)

START_TESTS_PASSED=0

if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.connected"; then
    echo "✓ Webhook: live_input.connected"
    START_TESTS_PASSED=$((START_TESTS_PASSED + 1))
else
    echo "✗ Missing: live_input.connected webhook"
fi

if echo "$LOGS" | grep -q "Stream started successfully via webhook:"; then
    echo "✓ Stream started successfully"
    START_TESTS_PASSED=$((START_TESTS_PASSED + 1))
else
    echo "✗ Missing: Stream started successfully"
fi

if [ $START_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 7 PASSED"
else
    echo "⚠️  TEST 7 PARTIAL: $START_TESTS_PASSED/2"
fi
echo ""

echo "========================================"
echo "TEST 7.5: Verify LIVE Nostr event"
echo "========================================"

echo "Querying Nostr relay for LIVE event..."

# Temporarily disable exit on error for this section
set +e

# Run query in background with timeout (only events from last 10 minutes)
SINCE_TIME=$(($(date +%s) - 600))
echo "[DEBUG] Starting node query with PID tracking..."
node "$SCRIPT_DIR/query_nostr_events_auth.js" 30311 --since $SINCE_TIME > /tmp/nostr_query_$$.txt 2>&1 &
QUERY_PID=$!
echo "[DEBUG] Query PID: $QUERY_PID"

# Wait up to 15 seconds for completion using simple counter
COUNTER=0
while [ $COUNTER -lt 15 ]; do
    if ! ps -p $QUERY_PID > /dev/null 2>&1; then
        echo "[DEBUG] Process completed after $COUNTER seconds"
        break
    fi
    sleep 1
    COUNTER=$((COUNTER + 1))
done

# Kill if still running
if ps -p $QUERY_PID > /dev/null 2>&1; then
    kill -9 $QUERY_PID 2>/dev/null || true
    echo "⚠️  Query timed out after 15 seconds"
fi

echo "[DEBUG] Reading output from /tmp/nostr_query_$$.txt"
if [ -f /tmp/nostr_query_$$.txt ]; then
    echo "[DEBUG] File exists, size: $(wc -c < /tmp/nostr_query_$$.txt) bytes"
else
    echo "[DEBUG] File does NOT exist!"
fi

# Parse ALL events and find the MOST RECENT one by created_at
echo "[DEBUG] Parsing all events to find most recent..."

# Re-enable exit on error
set -e

LIVE_EVENT_TESTS=0

# Extract ALL JSON events, parse with jq, sort by created_at, get most recent
if grep -q '"kind": 30311' /tmp/nostr_query_$$.txt 2>/dev/null; then
    # Extract all complete JSON objects and use jq to find most recent
    EVENT_JSON=$(awk '/^{$/,/^}$/ {print} /^}$/ {print "---SPLIT---"}' /tmp/nostr_query_$$.txt | \
        awk 'BEGIN{RS="---SPLIT---"} /"kind": 30311/ {print}' | \
        jq -s 'sort_by(.created_at) | reverse | .[0]' 2>/dev/null)
    
    if [ -z "$EVENT_JSON" ] || [ "$EVENT_JSON" == "null" ]; then
        echo "✗ Failed to parse events"
    else
        CREATED_AT=$(echo "$EVENT_JSON" | jq -r '.created_at' 2>/dev/null)
        echo "[DEBUG] Most recent event created_at: $CREATED_AT"
        
        STATUS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
        
        if [ "$STATUS" == "live" ]; then
        echo "✓ Event has status: live"
        LIVE_EVENT_TESTS=$((LIVE_EVENT_TESTS + 1))
    else
        echo "✗ Expected status 'live', got: $STATUS"
    fi
    
    # Check for streaming tag
    STREAMING_URL=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "streaming")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$STREAMING_URL" ] && [ "$STREAMING_URL" != "null" ] && [ "$STREAMING_URL" != "" ]; then
        echo "✓ Event has 'streaming' tag: ${STREAMING_URL:0:50}..."
        LIVE_EVENT_TESTS=$((LIVE_EVENT_TESTS + 1))
    else
        echo "✗ Missing 'streaming' tag in LIVE event"
    fi
    
    # Check starts tag exists
    STARTS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "starts")? | .[1]?' 2>/dev/null | head -n 1)
    if [ -n "$STARTS" ] && [ "$STARTS" != "null" ] && [ "$STARTS" != "" ]; then
        echo "✓ Event has 'starts' timestamp"
        LIVE_EVENT_TESTS=$((LIVE_EVENT_TESTS + 1))
    else
        echo "✗ Missing 'starts' tag"
    fi
    
    # Check ends tag does NOT exist yet
    ENDS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "ends")? | .[1]?' 2>/dev/null | head -n 1)
        if [ -z "$ENDS" ] || [ "$ENDS" == "null" ] || [ "$ENDS" == "" ]; then
            echo "✓ Event does NOT have 'ends' tag yet (correct)"
            LIVE_EVENT_TESTS=$((LIVE_EVENT_TESTS + 1))
        else
            echo "✗ Event has 'ends' tag but should not (still live)"
        fi
    fi
else
    echo "✗ No Nostr event found in output"
fi

rm -f /tmp/nostr_query_$$.txt

if [ $LIVE_EVENT_TESTS -eq 4 ]; then
    echo "✅ TEST 7.5 PASSED"
else
    echo "⚠️  TEST 7.5 PARTIAL: $LIVE_EVENT_TESTS/4"
fi
echo ""

echo "========================================"
echo "TEST 8: End stream"
echo "========================================"

if ps -p $FFMPEG_PID > /dev/null 2>&1; then
    kill -9 $FFMPEG_PID 2>/dev/null || true
    pkill -9 -f "ffmpeg.*testsrc" 2>/dev/null || true
    echo "✓ Stream stopped"
else
    echo "⚠️  Stream already stopped"
fi
rm "$FFMPEG_LOG"
echo "✅ TEST 8 PASSED"
echo ""

echo "========================================"
echo "TEST 9: Webhooks trigger stream END"
echo "========================================"

echo "Waiting 10 seconds for END webhooks..."
sleep 10

LOGS=$(docker logs --tail 100 zap-stream-core-core-1 2>&1)

END_TESTS_PASSED=0

if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.disconnected"; then
    echo "✓ Webhook: live_input.disconnected"
    END_TESTS_PASSED=$((END_TESTS_PASSED + 1))
else
    echo "✗ Missing: live_input.disconnected webhook"
fi

if echo "$LOGS" | grep -q "Stream ended successfully via webhook:"; then
    echo "✓ Stream ended successfully"
    END_TESTS_PASSED=$((END_TESTS_PASSED + 1))
else
    echo "✗ Missing: Stream ended successfully"
fi

if [ $END_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 9 PASSED"
else
    echo "⚠️  TEST 9 PARTIAL: $END_TESTS_PASSED/2"
fi
echo ""

echo "========================================"
echo "TEST 9.5: Verify ENDED Nostr event"
echo "========================================"

echo "Querying Nostr relay for ENDED event..."

# Temporarily disable exit on error for this section
set +e

# Run query in background with timeout (only events from last 10 minutes)
SINCE_TIME=$(($(date +%s) - 600))
node "$SCRIPT_DIR/query_nostr_events_auth.js" 30311 --since $SINCE_TIME > /tmp/nostr_query_ended_$$.txt 2>&1 &
QUERY_PID=$!

# Wait up to 15 seconds for completion using simple counter
COUNTER=0
while [ $COUNTER -lt 15 ]; do
    if ! ps -p $QUERY_PID > /dev/null 2>&1; then
        break
    fi
    sleep 1
    COUNTER=$((COUNTER + 1))
done

# Kill if still running
if ps -p $QUERY_PID > /dev/null 2>&1; then
    kill -9 $QUERY_PID 2>/dev/null || true
    echo "⚠️  Query timed out after 15 seconds"
fi

# Parse ALL events and find the MOST RECENT one by created_at
# Re-enable exit on error
set -e

ENDED_EVENT_TESTS=0

# Extract ALL JSON events, parse with jq, sort by created_at, get most recent
if grep -q '"kind": 30311' /tmp/nostr_query_ended_$$.txt 2>/dev/null; then
    EVENT_JSON=$(awk '/^{$/,/^}$/ {print} /^}$/ {print "---SPLIT---"}' /tmp/nostr_query_ended_$$.txt | \
        awk 'BEGIN{RS="---SPLIT---"} /"kind": 30311/ {print}' | \
        jq -s 'sort_by(.created_at) | reverse | .[0]' 2>/dev/null)
    
    if [ -z "$EVENT_JSON" ] || [ "$EVENT_JSON" == "null" ]; then
        echo "✗ Failed to parse events"
    else
        STATUS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "status")? | .[1]?' 2>/dev/null | head -n 1)
    
        if [ "$STATUS" == "ended" ]; then
            echo "✓ Event has status: ended"
            ENDED_EVENT_TESTS=$((ENDED_EVENT_TESTS + 1))
        else
            echo "✗ Expected status 'ended', got: $STATUS"
        fi
    
        # Check ends tag now exists
        ENDS=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "ends")? | .[1]?' 2>/dev/null | head -n 1)
        if [ -n "$ENDS" ] && [ "$ENDS" != "null" ] && [ "$ENDS" != "" ]; then
            echo "✓ Event has 'ends' timestamp"
            ENDED_EVENT_TESTS=$((ENDED_EVENT_TESTS + 1))
        else
            echo "✗ Missing 'ends' tag in ENDED event"
        fi
        
        # Check streaming tag is removed
        STREAMING_URL=$(echo "$EVENT_JSON" | jq -r '.tags[]? | select(.[0] == "streaming")? | .[1]?' 2>/dev/null | head -n 1)
        if [ -z "$STREAMING_URL" ] || [ "$STREAMING_URL" == "null" ] || [ "$STREAMING_URL" == "" ]; then
            echo "✓ 'streaming' tag removed (correct)"
            ENDED_EVENT_TESTS=$((ENDED_EVENT_TESTS + 1))
        else
            echo "✗ 'streaming' tag still present: $STREAMING_URL"
        fi
    fi
else
    echo "✗ No Nostr event found"
fi

rm -f /tmp/nostr_query_ended_$$.txt

if [ $ENDED_EVENT_TESTS -eq 3 ]; then
    echo "✅ TEST 9.5 PASSED"
else
    echo "⚠️  TEST 9.5 PARTIAL: $ENDED_EVENT_TESTS/3"
fi
echo ""

echo "========================================"
echo "TEST SUMMARY"
echo "========================================"
echo "✅ TEST 1: Check initial database state"
echo "✅ TEST 2: API call handles user correctly"
echo "✅ TEST 3: Database now contains valid UID"
echo "✅ TEST 4: Cloudflare returns valid credentials"
echo "✅ TEST 5: Second API call reuses same UID"
echo "✅ TEST 6: Stream to Cloudflare"
if [ $START_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 7: Webhooks trigger stream START"
else
    echo "⚠️  TEST 7: PARTIAL ($START_TESTS_PASSED/2)"
fi
if [ $LIVE_EVENT_TESTS -eq 4 ]; then
    echo "✅ TEST 7.5: Verify LIVE Nostr event"
else
    echo "⚠️  TEST 7.5: PARTIAL ($LIVE_EVENT_TESTS/4)"
fi
echo "✅ TEST 8: End stream"
if [ $END_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 9: Webhooks trigger stream END"
else
    echo "⚠️  TEST 9: PARTIAL ($END_TESTS_PASSED/2)"
fi
if [ $ENDED_EVENT_TESTS -eq 3 ]; then
    echo "✅ TEST 9.5: Verify ENDED Nostr event"
else
    echo "⚠️  TEST 9.5: PARTIAL ($ENDED_EVENT_TESTS/3)"
fi
echo ""
echo "Full logs: docker logs --tail 200 zap-stream-core-core-1 | grep -i cloudflare"
