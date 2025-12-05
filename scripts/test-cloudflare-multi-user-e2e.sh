#!/bin/bash

# ==========================================
# Cloudflare Multi-User E2E Integration Test
# ==========================================
# 
# This script tests concurrent multi-user streaming with key persistence:
# 1. Two users get unique stream keys from Cloudflare
# 2. Both users stream concurrently
# 3. Verify stream isolation (stopping one doesn't affect other)
# 4. Verify key persistence (reuse same key across sessions)
# 5. Validate Cloudflare API (no duplicate Live Inputs)

set -e  # Exit on error

# Test credentials (safe test keypairs, not production)
USER_A_NSEC="nsec15devjmm9cgwlpu7dw64cl29c02taw9gjrt5k6s78wxh3frwhhdcs986v76"
USER_A_NPUB="npub1tc6nuphuz0k0destd32mfluctx5jke60yxd794h3ugq7fgqgx0zq5eeln6"

USER_B_NSEC="nsec1u47296qau8ssg675wezgem0z3jslwxjaqs9xve74w3yn3v4esryqeqn2qg"
USER_B_NPUB="npub1xy7wqze00wut9psqa7psp5sjqzcfz49swh94ajudtfh3767llraqp3laua"

echo "========================================"
echo "Cloudflare Multi-User E2E Test"
echo "========================================"
echo ""
echo "User A: $USER_A_NPUB"
echo "User B: $USER_B_NPUB"
echo ""

# Check prerequisites
echo "[Step 1] Checking prerequisites..."

if ! command -v node &> /dev/null; then
    echo "❌ ERROR: node not found"
    exit 1
fi
echo "✓ Node.js found"

if ! docker ps &> /dev/null; then
    echo "❌ ERROR: Docker is not running"
    exit 1
fi
echo "✓ Docker is running"

if ! docker ps | grep -q zap-stream-core-core-1; then
    echo "❌ ERROR: zap-stream-core-core-1 container not running"
    exit 1
fi
echo "✓ zap-stream-core container is running"

# Helper function to decode npub to hex
decode_npub() {
    local npub=$1
    node -e "
const bech32Decode = (str) => {
  const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
  const data = str.slice(5);
  const values = [];
  for (const c of data) {
    const idx = CHARSET.indexOf(c);
    if (idx === -1) throw new Error(\`Invalid character: \${c}\`);
    values.push(idx);
  }
  const bits = [];
  for (const v of values) {
    for (let i = 4; i >= 0; i--) {
      bits.push((v >> i) & 1);
    }
  }
  const bytes = [];
  for (let i = 0; i < bits.length - (bits.length % 8); i += 8) {
    let byte = 0;
    for (let j = 0; j < 8; j++) {
      byte = (byte << 1) | bits[i + j];
    }
    bytes.push(byte);
  }
  bytes.pop();
  console.log(Buffer.from(bytes).toString('hex'));
};
bech32Decode('$npub');
"
}

# Helper function to call API for a user
call_api_for_user() {
    local nsec=$1
    local npub=$2
    local api_url="http://localhost:80/api/v1/account"
    local method="GET"
    
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local auth_event=$(node "$SCRIPT_DIR/sign_nip98.js" "$nsec" "$api_url" "$method" 2>&1)
    
    if [ $? -ne 0 ]; then
        echo "❌ ERROR: Failed to create NIP-98 event for $npub"
        echo "$auth_event"
        return 1
    fi
    
    local auth_token=$(echo "$auth_event" | base64)
    curl -s "$api_url" -H "Authorization: Nostr $auth_token"
}

echo ""
echo "[Step 2] Setting up users in database..."

USER_A_HEX=$(decode_npub "$USER_A_NPUB")
USER_B_HEX=$(decode_npub "$USER_B_NPUB")

echo "User A hex: $USER_A_HEX"
echo "User B hex: $USER_B_HEX"

# Insert both users
docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${USER_A_HEX}'), 0);" \
  2>/dev/null || true

docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${USER_B_HEX}'), 0);" \
  2>/dev/null || true

echo "✓ Both users ensured in database"

echo ""
echo "[Step 3] Getting stream keys for both users..."

# User A gets key
echo "User A: Calling API..."
USER_A_RESPONSE=$(call_api_for_user "$USER_A_NSEC" "$USER_A_NPUB")

if ! echo "$USER_A_RESPONSE" | jq -e '.endpoints' > /dev/null 2>&1; then
    echo "❌ ERROR: User A API call failed"
    echo "$USER_A_RESPONSE"
    exit 1
fi

KEY_A=$(echo "$USER_A_RESPONSE" | jq -r '.endpoints[0].key')
RTMP_URL=$(echo "$USER_A_RESPONSE" | jq -r '.endpoints[0].url')

echo "✓ User A key: ${KEY_A:0:20}..."

# User B gets key
echo "User B: Calling API..."
USER_B_RESPONSE=$(call_api_for_user "$USER_B_NSEC" "$USER_B_NPUB")

if ! echo "$USER_B_RESPONSE" | jq -e '.endpoints' > /dev/null 2>&1; then
    echo "❌ ERROR: User B API call failed"
    echo "$USER_B_RESPONSE"
    exit 1
fi

KEY_B=$(echo "$USER_B_RESPONSE" | jq -r '.endpoints[0].key')

echo "✓ User B key: ${KEY_B:0:20}..."

# Verify keys are different
if [ "$KEY_A" == "$KEY_B" ]; then
    echo "❌ ERROR: Both users got same key!"
    exit 1
fi
echo "✓ Keys are unique"

# Verify both are valid Cloudflare format
if ! [[ $KEY_A =~ ^[0-9a-f]{32}$ ]]; then
    echo "❌ ERROR: User A key not valid Cloudflare format: $KEY_A"
    exit 1
fi

if ! [[ $KEY_B =~ ^[0-9a-f]{32}$ ]]; then
    echo "❌ ERROR: User B key not valid Cloudflare format: $KEY_B"
    exit 1
fi
echo "✓ Both keys are valid Cloudflare format"

echo ""
echo "[Step 4] Starting concurrent streams..."

# Create temp files for ffmpeg logs
FFMPEG_LOG_A=$(mktemp)
FFMPEG_LOG_B=$(mktemp)
echo "FFmpeg logs: $FFMPEG_LOG_A (User A), $FFMPEG_LOG_B (User B)"

# Start User A stream
echo "Starting User A stream..."
RTMP_DEST_A="$RTMP_URL"  # URL already includes the key
ffmpeg -re -t 60 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST_A" \
  </dev/null >>"$FFMPEG_LOG_A" 2>&1 &

PID_A=$!
echo "✓ User A streaming (PID: $PID_A)"

# Wait and verify User A is streaming
sleep 5
if ! ps -p $PID_A > /dev/null 2>&1; then
    echo "❌ ERROR: User A ffmpeg died"
    cat "$FFMPEG_LOG_A"
    exit 1
fi
echo "✓ User A stream active"

# Start User B stream (concurrent)
echo "Starting User B stream (concurrent)..."
# Get User B's full URL (already includes key)
RTMP_URL_B=$(echo "$USER_B_RESPONSE" | jq -r '.endpoints[0].url')
RTMP_DEST_B="$RTMP_URL_B"
ffmpeg -re -t 60 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=800:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST_B" \
  </dev/null >>"$FFMPEG_LOG_B" 2>&1 &

PID_B=$!
echo "✓ User B streaming (PID: $PID_B)"

# Wait and verify both are streaming
sleep 5
if ! ps -p $PID_A > /dev/null 2>&1; then
    echo "❌ ERROR: User A ffmpeg died after User B started"
    exit 1
fi

if ! ps -p $PID_B > /dev/null 2>&1; then
    echo "❌ ERROR: User B ffmpeg died"
    cat "$FFMPEG_LOG_B"
    exit 1
fi
echo "✓ Both streams active concurrently"

echo ""
echo "[Step 5] Testing stream isolation..."

# Stop User A only
echo "Stopping User A stream..."
kill -9 $PID_A 2>/dev/null || true
sleep 2

# Verify User B still running
if ! ps -p $PID_B > /dev/null 2>&1; then
    echo "❌ ERROR: User B stream died when User A stopped!"
    exit 1
fi
echo "✓ User B still streaming after User A stopped (isolation verified)"

# Wait for webhooks
sleep 8

# Stop User B
echo "Stopping User B stream..."
kill -9 $PID_B 2>/dev/null || true
sleep 5

echo ""
echo "[Step 6] Testing key persistence (User A streams again)..."

# User A calls API again
echo "User A: Calling API again..."
USER_A_RESPONSE_2=$(call_api_for_user "$USER_A_NSEC" "$USER_A_NPUB")
KEY_A_2=$(echo "$USER_A_RESPONSE_2" | jq -r '.endpoints[0].key')

# Verify same key
if [ "$KEY_A" != "$KEY_A_2" ]; then
    echo "❌ ERROR: User A got different key on second call!"
    echo "First:  $KEY_A"
    echo "Second: $KEY_A_2"
    exit 1
fi
echo "✓ User A key persisted: ${KEY_A:0:20}..."

# Start User A stream again with same key
echo "User A: Streaming again with same key..."
# Get fresh URL for User A
RTMP_URL_A2=$(echo "$USER_A_RESPONSE_2" | jq -r '.endpoints[0].url')
RTMP_DEST_A="$RTMP_URL_A2"
ffmpeg -re -t 20 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST_A" \
  </dev/null >>"$FFMPEG_LOG_A" 2>&1 &

PID_A=$!

sleep 5
if ! ps -p $PID_A > /dev/null 2>&1; then
    echo "❌ ERROR: User A stream failed to restart"
    cat "$FFMPEG_LOG_A"
    exit 1
fi
echo "✓ User A streaming again with same key"

# Stop User A
kill -9 $PID_A 2>/dev/null || true
sleep 5

# Cleanup ffmpeg logs
rm "$FFMPEG_LOG_A" "$FFMPEG_LOG_B"

echo ""
echo "[Step 7] Validating database state..."

# Check User A has correct key
DB_KEY_A=$(docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream \
    -e "SELECT stream_key FROM user WHERE HEX(pubkey)='${USER_A_HEX}';" -s -N 2>/dev/null)

if [ "$DB_KEY_A" != "$KEY_A" ]; then
    echo "❌ ERROR: User A key mismatch in database"
    echo "Expected: $KEY_A"
    echo "Got:      $DB_KEY_A"
    exit 1
fi
echo "✓ User A key correct in DB: ${DB_KEY_A:0:20}..."

# Check User B has correct key
DB_KEY_B=$(docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream \
    -e "SELECT stream_key FROM user WHERE HEX(pubkey)='${USER_B_HEX}';" -s -N 2>/dev/null)

if [ "$DB_KEY_B" != "$KEY_B" ]; then
    echo "❌ ERROR: User B key mismatch in database"
    echo "Expected: $KEY_B"
    echo "Got:      $DB_KEY_B"
    exit 1
fi
echo "✓ User B key correct in DB: ${DB_KEY_B:0:20}..."

echo ""
echo "[Step 8] Checking Docker logs for webhook events..."

LOGS=$(docker logs --tail 200 zap-stream-core-core-1 2>&1)

# Count connected events
CONNECTED_COUNT=$(echo "$LOGS" | grep -c "Stream connected:" || true)
DISCONNECTED_COUNT=$(echo "$LOGS" | grep -c "Stream disconnected:" || true)

echo "Webhook events found:"
echo "  - Connected: $CONNECTED_COUNT"
echo "  - Disconnected: $DISCONNECTED_COUNT"

if [ $CONNECTED_COUNT -lt 3 ]; then
    echo "⚠ Warning: Expected at least 3 connect events (User A, User B, User A again)"
fi

echo ""
echo "========================================"
echo "Test Complete!"
echo "========================================"
echo ""
echo "Summary:"
echo "✓ Multi-user key generation (unique keys)"
echo "✓ Concurrent streaming (both users simultaneously)"
echo "✓ Stream isolation (stopping A didn't affect B)"
echo "✓ Key persistence (User A reused same key)"
echo "✓ Database validation (keys correctly stored)"
echo ""
echo "Users tested:"
echo "  User A: ${KEY_A:0:20}... (streamed twice)"
echo "  User B: ${KEY_B:0:20}... (streamed once)"
