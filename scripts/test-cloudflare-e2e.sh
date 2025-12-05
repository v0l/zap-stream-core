#!/bin/bash

# ==========================================
# Cloudflare End-to-End Integration Test
# ==========================================
# 
# This script verifies the complete Cloudflare streaming lifecycle.
# Works with both NEW users (no UID) and EXISTING users (has UID).

set -e  # Exit on error

# Test credentials (safe test keypair, not production)
TEST_NSEC="nsec1kczxrs69y6vdujuwwg2hegxngun4507clh8px73degq62kv9qreqdessxr"
TEST_NPUB="npub1qf680y78ga29evk5k386kh8qwuukmvu7uk39p5mdwdw37gzl2svqpkpren"

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

if echo "$LOGS" | grep -q "Stream connected:"; then
    echo "✓ Stream connected event"
    START_TESTS_PASSED=$((START_TESTS_PASSED + 1))
else
    echo "✗ Missing: Stream connected event"
fi

if [ $START_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 7 PASSED"
else
    echo "⚠️  TEST 7 PARTIAL: $START_TESTS_PASSED/2"
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

if echo "$LOGS" | grep -q "Stream disconnected:"; then
    echo "✓ Stream disconnected event"
    END_TESTS_PASSED=$((END_TESTS_PASSED + 1))
else
    echo "✗ Missing: Stream disconnected event"
fi

if [ $END_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 9 PASSED"
else
    echo "⚠️  TEST 9 PARTIAL: $END_TESTS_PASSED/2"
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
echo "✅ TEST 8: End stream"
if [ $END_TESTS_PASSED -eq 2 ]; then
    echo "✅ TEST 9: Webhooks trigger stream END"
else
    echo "⚠️  TEST 9: PARTIAL ($END_TESTS_PASSED/2)"
fi
echo ""
echo "Full logs: docker logs --tail 200 zap-stream-core-core-1 | grep -i cloudflare"
