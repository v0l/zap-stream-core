#!/bin/bash

# ==========================================
# Minimal Cloudflare Streaming Test
# ==========================================
# 
# This test ONLY verifies that FFmpeg can connect and stream to Cloudflare.
# No webhooks, no lifecycle checks, no database validation.
# Just: Can we push video data to Cloudflare's RTMPS endpoint?

set -e

# Test credentials (safe test keypair)
TEST_NSEC="nsec15devjmm9cgwlpu7dw64cl29c02taw9gjrt5k6s78wxh3frwhhdcs986v76"
TEST_NPUB="npub1tc6nuphuz0k0destd32mfluctx5jke60yxd794h3ugq7fgqgx0zq5eeln6"

echo "========================================"
echo "Minimal Cloudflare Streaming Test"
echo "========================================"
echo ""
echo "Purpose: Verify FFmpeg can stream to Cloudflare"
echo "Test: 10-second stream to verify connection"
echo ""

# Check FFmpeg
if ! command -v ffmpeg &> /dev/null; then
    echo "❌ ERROR: ffmpeg not found"
    exit 1
fi
echo "✓ FFmpeg found"

# Check Docker
if ! docker ps | grep -q zap-stream-core-core-1; then
    echo "❌ ERROR: zap-stream-core container not running"
    exit 1
fi
echo "✓ Docker container running"

echo ""
echo "[Step 1] Getting stream key from API..."

# API endpoint
API_URL="http://localhost:80/api/v1/account"
METHOD="GET"

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Create NIP-98 auth event
AUTH_EVENT_JSON=$(node "$SCRIPT_DIR/sign_nip98.js" "$TEST_NSEC" "$API_URL" "$METHOD" 2>&1)

if [ $? -ne 0 ]; then
    echo "❌ ERROR: Failed to create NIP-98 event"
    exit 1
fi

# Call API
AUTH_TOKEN=$(echo "$AUTH_EVENT_JSON" | base64)
API_RESPONSE=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_TOKEN")

# Extract RTMP URL from the Cloudflare-Basic endpoint (free tier)
RTMP_URL=$(echo "$API_RESPONSE" | jq -r '.endpoints[] | select(.name=="Cloudflare-Basic") | .url' 2>/dev/null)
STREAM_KEY=$(echo "$API_RESPONSE" | jq -r '.endpoints[] | select(.name=="Cloudflare-Basic") | .key' 2>/dev/null)

if [ -z "$RTMP_URL" ] || [ -z "$STREAM_KEY" ]; then
    echo "❌ ERROR: Could not extract RTMP URL or key from API"
    echo "API Response:"
    echo "$API_RESPONSE" | jq '.'
    exit 1
fi

echo "✓ Got stream credentials from API"
echo "  URL: $RTMP_URL"
echo "  Key: ${STREAM_KEY:0:20}..."

# Construct full streaming URL
STREAM_DEST="${RTMP_URL}"

echo ""
echo "[Step 2] Testing stream connection..."
echo "  Destination: $STREAM_DEST"
echo ""

# Create temp file for ffmpeg output
FFMPEG_LOG=$(mktemp)
echo "FFmpeg log: $FFMPEG_LOG"

# Stream for 10 seconds with verbose output
echo "Starting 10-second test stream..."
ffmpeg -re -t 10 \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -b:v 2000k \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv "$STREAM_DEST" \
  </dev/null >"$FFMPEG_LOG" 2>&1

FFMPEG_EXIT_CODE=$?

echo ""
echo "[Step 3] Analyzing results..."
echo ""

# Show last 30 lines of output
echo "FFmpeg output (last 30 lines):"
echo "----------------------------------------"
tail -30 "$FFMPEG_LOG"
echo "----------------------------------------"
echo ""

# Check exit code
if [ $FFMPEG_EXIT_CODE -eq 0 ]; then
    echo "✅ SUCCESS: FFmpeg completed without errors (exit code 0)"
else
    echo "❌ FAILED: FFmpeg exited with code $FFMPEG_EXIT_CODE"
fi

# Check for specific errors
if grep -q "Broken pipe" "$FFMPEG_LOG"; then
    echo "❌ FOUND: 'Broken pipe' error - connection rejected"
fi

if grep -q "Error in the push function" "$FFMPEG_LOG"; then
    echo "❌ FOUND: TLS push error - RTMPS connection failed"
fi

if grep -q "frame=" "$FFMPEG_LOG" | tail -1 | grep -q "frame=.*[1-9]"; then
    FRAME_COUNT=$(grep "frame=" "$FFMPEG_LOG" | tail -1 | sed -n 's/.*frame=\s*\([0-9]*\).*/\1/p')
    echo "✓ Frames encoded: $FRAME_COUNT"
    if [ "$FRAME_COUNT" -gt 100 ]; then
        echo "✅ SUCCESS: Significant data was streamed ($FRAME_COUNT frames)"
    fi
fi

# Cleanup
rm "$FFMPEG_LOG"

echo ""
echo "========================================"
echo "Test Complete"
echo "========================================"
echo ""

if [ $FFMPEG_EXIT_CODE -eq 0 ]; then
    echo "✅ RESULT: Streaming to Cloudflare WORKS"
    exit 0
else
    echo "❌ RESULT: Streaming to Cloudflare FAILED"
    echo ""
    echo "Possible causes:"
    echo "  - RTMPS URL format incorrect"
    echo "  - Stream key invalid"
    echo "  - Cloudflare account/credentials issue"
    echo "  - Recent code changes broke streaming"
    exit 1
fi
