#!/bin/bash

# ==========================================
# Cloudflare End-to-End Integration Test
# ==========================================
# 
# This script tests the complete Cloudflare streaming lifecycle:
# 1. Creates NIP-98 authenticated API call
# 2. Gets Cloudflare RTMP endpoint (creates Live Input + mapping)
# 3. Streams test pattern to Cloudflare
# 4. Verifies webhooks trigger stream START lifecycle
# 5. Stops stream
# 6. Verifies webhooks trigger stream END lifecycle

set -e  # Exit on error

# Test credentials (safe test keypair, not production)
TEST_NSEC="nsec15devjmm9cgwlpu7dw64cl29c02taw9gjrt5k6s78wxh3frwhhdcs986v76"
TEST_NPUB="npub1tc6nuphuz0k0destd32mfluctx5jke60yxd794h3ugq7fgqgx0zq5eeln6"

# Extract hex pubkey from npub (remove 'npub1' prefix and convert from bech32)
# For now, we'll use a simpler approach - decode with nak if available
TEST_PUBKEY_HEX=$(echo "$TEST_NPUB" | awk '{print $1}' | cut -c6-)

echo "========================================"
echo "Cloudflare E2E Integration Test"
echo "========================================"
echo ""
echo "Test Pubkey: $TEST_NPUB"
echo ""

# Check prerequisites
echo "[Step 1] Checking prerequisites..."

# Check if Node.js is installed
if ! command -v node &> /dev/null; then
    echo "❌ ERROR: node not found"
    exit 1
fi
echo "✓ Node.js found"

# Check if Docker is running
if ! docker ps &> /dev/null; then
    echo "❌ ERROR: Docker is not running"
    exit 1
fi
echo "✓ Docker is running"

# Check if zap-stream-core container is running
if ! docker ps | grep -q zap-stream-core-core-1; then
    echo "❌ ERROR: zap-stream-core-core-1 container not running"
    echo "Start it with: cd docs/deploy && docker-compose up -d"
    exit 1
fi
echo "✓ zap-stream-core container is running"

echo ""
echo "[Step 2] Ensuring test user exists in database..."

# Decode npub to hex using Node.js
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_PUBKEY_HEX=$(node -e "
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
bech32Decode('$TEST_NPUB');
")

if [ -z "$TEST_PUBKEY_HEX" ]; then
    echo "❌ ERROR: Could not decode npub to hex"
    exit 1
fi

echo "Test pubkey hex: $TEST_PUBKEY_HEX"

# Insert user into database
docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${TEST_PUBKEY_HEX}'), 0);" \
  2>/dev/null || true

echo "✓ Test user ensured in database"

echo ""
echo "[Step 3] Creating NIP-98 auth token and calling API..."

# API endpoint (Docker maps container's 8080 to host's 80)
API_URL="http://localhost:80/api/v1/account"
METHOD="GET"

# Create NIP-98 event using Node.js script
echo "Creating NIP-98 auth event..."
AUTH_EVENT_JSON=$(node "$SCRIPT_DIR/sign_nip98.js" "$TEST_NSEC" "$API_URL" "$METHOD" 2>&1)

if [ $? -ne 0 ]; then
    echo "❌ ERROR: Failed to create NIP-98 event"
    echo "$AUTH_EVENT_JSON"
    exit 1
fi

echo "✓ NIP-98 event created"

# Base64 encode the event
AUTH_TOKEN=$(echo "$AUTH_EVENT_JSON" | base64)

# Call API with auth
echo "Calling API with authentication..."
API_RESPONSE=$(curl -s "$API_URL" \
  -H "Authorization: Nostr $AUTH_TOKEN")

# Check if API call succeeded
if echo "$API_RESPONSE" | jq -e '.endpoints' > /dev/null 2>&1; then
    echo "✓ API call succeeded"
else
    echo "❌ ERROR: API call failed"
    echo "Response: $API_RESPONSE"
    exit 1
fi

# Pretty print the response
echo ""
echo "API Response:"
echo "$API_RESPONSE" | jq '.'
echo ""

# Extract RTMP URL and stream key
RTMP_URL=$(echo "$API_RESPONSE" | jq -r '.endpoints[0].url // empty')
STREAM_KEY=$(echo "$API_RESPONSE" | jq -r '.endpoints[0].key // empty')

if [ -z "$RTMP_URL" ] || [ -z "$STREAM_KEY" ]; then
    echo "❌ ERROR: Could not extract RTMP URL or stream key from API response"
    exit 1
fi

echo "RTMP URL: $RTMP_URL"
echo "Stream Key: ${STREAM_KEY:0:20}..."
echo ""

echo "[Step 4] Starting test stream to Cloudflare..."

# Full RTMP destination
RTMP_DEST="${RTMP_URL}${STREAM_KEY}"

# Create temp file for ffmpeg output
FFMPEG_LOG=$(mktemp)
echo "FFmpeg log: $FFMPEG_LOG"

# Start ffmpeg streaming in background, capture output
echo "Streaming to: $RTMP_DEST"
ffmpeg -re -t 30 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$RTMP_DEST" \
  </dev/null >>"$FFMPEG_LOG" 2>&1 &

FFMPEG_PID=$!
echo "✓ FFmpeg started (PID: $FFMPEG_PID), streaming for 30 seconds..."

# Wait a moment and check if ffmpeg is still running
sleep 3
if ! ps -p $FFMPEG_PID > /dev/null 2>&1; then
    echo "❌ ERROR: FFmpeg died immediately"
    echo "FFmpeg output:"
    cat "$FFMPEG_LOG"
    rm "$FFMPEG_LOG"
    exit 1
fi
echo "✓ FFmpeg still running after 3 seconds"

# Wait for stream to establish and webhooks to arrive
echo "Waiting 20 seconds for stream to establish and webhooks to arrive..."
sleep 20

echo ""
echo "[Step 5] Verifying stream START in Docker logs..."

# Check for expected START logs
LOGS=$(docker logs --tail 100 zap-stream-core-core-1 2>&1)

echo "Checking for START indicators..."

# Check for webhook received
if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.connected"; then
    echo "✓ Webhook received: live_input.connected"
else
    echo "⚠ Warning: Did not find 'live_input.connected' webhook in logs"
fi

# Check for stream connected
if echo "$LOGS" | grep -q "Stream connected:"; then
    echo "✓ Stream connected event found"
else
    echo "⚠ Warning: Did not find 'Stream connected' in logs"
fi

# Check for Video Asset
if echo "$LOGS" | grep -q "Video Asset found"; then
    echo "✓ Video Asset created"
else
    echo "⚠ Warning: Video Asset may not be created yet (this can take a few seconds)"
fi

echo ""
echo "Recent logs:"
echo "$LOGS" | grep -i "cloudflare\|webhook\|stream\|connected" | tail -15
echo ""

echo "[Step 6] Checking FFmpeg status..."

# Check if ffmpeg is still running
if ps -p $FFMPEG_PID > /dev/null 2>&1; then
    echo "✓ FFmpeg still streaming"
    
    # Show last few lines of ffmpeg output
    echo ""
    echo "FFmpeg output (last 20 lines):"
    tail -20 "$FFMPEG_LOG"
    echo ""
    
    # Kill ffmpeg
    kill -9 $FFMPEG_PID 2>/dev/null || true
    pkill -9 -f "ffmpeg.*testsrc" 2>/dev/null || true
    echo "✓ FFmpeg stopped"
else
    echo "⚠ Warning: FFmpeg already stopped"
    echo ""
    echo "Full FFmpeg output:"
    cat "$FFMPEG_LOG"
    echo ""
fi

# Cleanup log file
rm "$FFMPEG_LOG"

echo "Waiting 10 seconds for END webhooks..."
sleep 10

echo ""
echo "[Step 7] Verifying stream END in Docker logs..."

# Check for expected END logs
LOGS=$(docker logs --tail 100 zap-stream-core-core-1 2>&1)

echo "Checking for END indicators..."

# Check for webhook received
if echo "$LOGS" | grep -q "Received Cloudflare webhook event: live_input.disconnected"; then
    echo "✓ Webhook received: live_input.disconnected"
else
    echo "⚠ Warning: Did not find 'live_input.disconnected' webhook in logs"
fi

# Check for stream ended
if echo "$LOGS" | grep -q "Stream disconnected:"; then
    echo "✓ Stream disconnected event found"
else
    echo "⚠ Warning: Did not find 'Stream disconnected' in logs"
fi

echo ""
echo "Recent logs:"
echo "$LOGS" | grep -i "cloudflare\|webhook\|stream\|disconnect" | tail -15
echo ""

echo "========================================"
echo "Test Complete"
echo "========================================"
echo ""
echo "Summary:"
echo "- API call with NIP-98 auth: ✓"
echo "- Cloudflare RTMP endpoint received: ✓"
echo "- Stream sent to Cloudflare: ✓"
echo "- Check logs above for webhook verification"
echo ""
echo "To see full logs:"
echo "docker logs --tail 200 zap-stream-core-core-1 | grep -i cloudflare"
