#!/bin/bash

# Test: Prove HLS URL discovery workflow
# 1. Create Live Input
# 2. Start streaming
# 3. Poll Videos API until asset appears
# 4. Extract HLS URL
# 5. Verify HLS URL works

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/.env"

echo -e "${BLUE}=== HLS URL Discovery Test ===${NC}\n"

# Step 1: Create Live Input
echo -e "${YELLOW}Step 1: Creating Live Input...${NC}"
CREATE_RESPONSE=$(curl -s -X POST \
  "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs" \
  -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "meta": {"name": "HLS Discovery Test"},
    "recording": {"mode": "automatic"}
  }')

LIVE_INPUT_UID=$(echo "$CREATE_RESPONSE" | jq -r '.result.uid')
RTMP_URL=$(echo "$CREATE_RESPONSE" | jq -r '.result.rtmps.url')
STREAM_KEY=$(echo "$CREATE_RESPONSE" | jq -r '.result.rtmps.streamKey')

echo -e "${GREEN}✓ Live Input Created${NC}"
echo "  UID: $LIVE_INPUT_UID"
echo "  RTMP: ${RTMP_URL}${STREAM_KEY}"

# Step 2: Start streaming in background
echo -e "\n${YELLOW}Step 2: Starting ffmpeg stream...${NC}"
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency -g 60 \
  -c:a aac -ar 44100 \
  -f flv "${RTMP_URL}${STREAM_KEY}" \
  </dev/null >/dev/null 2>&1 &

FFMPEG_PID=$!
echo -e "${GREEN}✓ Stream started (PID: $FFMPEG_PID)${NC}"

# Cleanup function
cleanup() {
  echo -e "\n${YELLOW}Cleaning up...${NC}"
  kill $FFMPEG_PID 2>/dev/null || true
  curl -s -X DELETE \
    "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}" \
    -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" > /dev/null
  echo -e "${GREEN}✓ Cleanup complete${NC}"
}
trap cleanup EXIT

# Step 3: Poll Videos API for asset
echo -e "\n${YELLOW}Step 3: Polling Videos API for asset creation...${NC}"
echo "  (This may take 10-30 seconds after streaming starts)"

MAX_ATTEMPTS=30
ATTEMPT=0
HLS_URL=""

while [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
  ATTEMPT=$((ATTEMPT + 1))
  echo -n "  Attempt $ATTEMPT/$MAX_ATTEMPTS..."
  
  ASSETS_RESPONSE=$(curl -s \
    "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream?liveInput=${LIVE_INPUT_UID}" \
    -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}")
  
  ASSET_COUNT=$(echo "$ASSETS_RESPONSE" | jq '.result | length')
  
  if [ "$ASSET_COUNT" -gt 0 ]; then
    HLS_URL=$(echo "$ASSETS_RESPONSE" | jq -r '.result[0].playback.hls')
    ASSET_UID=$(echo "$ASSETS_RESPONSE" | jq -r '.result[0].uid')
    echo -e " ${GREEN}Found!${NC}"
    echo -e "${GREEN}✓ Video Asset Created${NC}"
    echo "  Asset UID: $ASSET_UID"
    echo "  HLS URL: $HLS_URL"
    break
  fi
  
  echo " not yet"
  sleep 2
done

if [ -z "$HLS_URL" ]; then
  echo -e "${RED}✗ Asset not created after ${MAX_ATTEMPTS} attempts${NC}"
  exit 1
fi

# Step 4: Verify HLS URL is accessible
echo -e "\n${YELLOW}Step 4: Verifying HLS URL is accessible...${NC}"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$HLS_URL")

if [ "$HTTP_CODE" = "200" ]; then
  echo -e "${GREEN}✓ HLS URL accessible (HTTP $HTTP_CODE)${NC}"
else
  echo -e "${RED}✗ HLS URL not accessible (HTTP $HTTP_CODE)${NC}"
  exit 1
fi

# Step 5: Download and verify HLS manifest
echo -e "\n${YELLOW}Step 5: Downloading HLS manifest...${NC}"
MANIFEST=$(curl -s "$HLS_URL")

if echo "$MANIFEST" | grep -q "#EXTM3U"; then
  echo -e "${GREEN}✓ Valid HLS manifest received${NC}"
  echo -e "\nFirst 10 lines of manifest:"
  echo "$MANIFEST" | head -10
else
  echo -e "${RED}✗ Invalid HLS manifest${NC}"
  exit 1
fi

# Success!
echo -e "\n${GREEN}=== SUCCESS ===${NC}"
echo "Architecture validated:"
echo "  1. ✓ Live Input created via API"
echo "  2. ✓ Streaming started to Live Input"
echo "  3. ✓ Video Asset auto-created by Cloudflare"
echo "  4. ✓ HLS URL obtained from Videos API"
echo "  5. ✓ HLS URL accessible and valid"
echo ""
echo "Implementation confirmed:"
echo "  - Query: GET /stream?liveInput={uid}"
echo "  - Response: result[0].playback.hls"
echo "  - HLS URL: $HLS_URL"
