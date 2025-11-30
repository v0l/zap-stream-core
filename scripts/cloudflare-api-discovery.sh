#!/bin/bash

# Cloudflare Stream API Discovery Script
# Purpose: Discover actual API response structure for HLS URLs and Live Input behavior
# Date: 2025-12-01

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Cloudflare Stream API Discovery ===${NC}\n"

# Load credentials from .env file if it exists
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/.env" ]; then
    echo -e "${GREEN}✓ Loading credentials from .env file${NC}"
    source "$SCRIPT_DIR/.env"
else
    echo -e "${YELLOW}⚠ No .env file found at $SCRIPT_DIR/.env${NC}"
    echo "Create one from .env.example:"
    echo "  cp $SCRIPT_DIR/.env.example $SCRIPT_DIR/.env"
    echo "Then edit .env with your Cloudflare credentials"
    echo ""
fi

# Check for required environment variables
if [ -z "$CLOUDFLARE_ACCOUNT_ID" ]; then
    echo -e "${RED}ERROR: CLOUDFLARE_ACCOUNT_ID not set${NC}"
    echo "Either:"
    echo "  1. Set in .env file (recommended)"
    echo "  2. Export: export CLOUDFLARE_ACCOUNT_ID='your-account-id'"
    exit 1
fi

if [ -z "$CLOUDFLARE_API_TOKEN" ]; then
    echo -e "${RED}ERROR: CLOUDFLARE_API_TOKEN not set${NC}"
    echo "Either:"
    echo "  1. Set in .env file (recommended)"
    echo "  2. Export: export CLOUDFLARE_API_TOKEN='your-api-token'"
    exit 1
fi

echo -e "${GREEN}✓ Environment variables configured${NC}"
echo -e "Account ID: ${CLOUDFLARE_ACCOUNT_ID}"
echo -e "API Token: ${CLOUDFLARE_API_TOKEN:0:20}...${NC}\n"

# Create output directory for responses
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTPUT_DIR="cloudflare-api-responses-${TIMESTAMP}"
mkdir -p "$OUTPUT_DIR"

echo -e "${BLUE}Responses will be saved to: ${OUTPUT_DIR}/${NC}\n"

# Test 1: Create Live Input
echo -e "${YELLOW}=== Test 1: Create Live Input ===${NC}"
echo "POST /accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs"

CREATE_RESPONSE=$(curl -s -X POST \
  "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs" \
  -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "meta": {
      "name": "API Discovery Test - '"${TIMESTAMP}"'"
    },
    "recording": {
      "mode": "automatic",
      "timeoutSeconds": 30
    }
  }')

echo "$CREATE_RESPONSE" | jq '.' > "${OUTPUT_DIR}/1-create-live-input.json"
echo -e "${GREEN}✓ Response saved to ${OUTPUT_DIR}/1-create-live-input.json${NC}"

# Check if creation was successful
SUCCESS=$(echo "$CREATE_RESPONSE" | jq -r '.success')
if [ "$SUCCESS" != "true" ]; then
    echo -e "${RED}ERROR: Failed to create Live Input${NC}"
    echo "$CREATE_RESPONSE" | jq '.'
    exit 1
fi

# Extract Live Input UID and RTMP credentials
LIVE_INPUT_UID=$(echo "$CREATE_RESPONSE" | jq -r '.result.uid')
RTMP_URL=$(echo "$CREATE_RESPONSE" | jq -r '.result.rtmps.url')
STREAM_KEY=$(echo "$CREATE_RESPONSE" | jq -r '.result.rtmps.streamKey')

echo -e "\n${GREEN}✓ Live Input Created${NC}"
echo -e "Live Input UID: ${LIVE_INPUT_UID}"
echo -e "RTMP URL: ${RTMP_URL}"
echo -e "Stream Key: ${STREAM_KEY:0:20}...${NC}\n"

# Save important values
cat > "${OUTPUT_DIR}/credentials.txt" << EOF
LIVE_INPUT_UID=${LIVE_INPUT_UID}
RTMP_URL=${RTMP_URL}
STREAM_KEY=${STREAM_KEY}
EOF

# Test 2: Get Live Input Details (Before Streaming)
echo -e "${YELLOW}=== Test 2: Get Live Input Details (Before Streaming) ===${NC}"
echo "GET /accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}"

sleep 2  # Give API time to fully create the resource

GET_RESPONSE=$(curl -s \
  "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}" \
  -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}")

echo "$GET_RESPONSE" | jq '.' > "${OUTPUT_DIR}/2-get-live-input-before-stream.json"
echo -e "${GREEN}✓ Response saved to ${OUTPUT_DIR}/2-get-live-input-before-stream.json${NC}\n"

# Test 3: List All Live Inputs
echo -e "${YELLOW}=== Test 3: List All Live Inputs ===${NC}"
echo "GET /accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs"

LIST_RESPONSE=$(curl -s \
  "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs" \
  -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}")

echo "$LIST_RESPONSE" | jq '.' > "${OUTPUT_DIR}/3-list-live-inputs.json"
echo -e "${GREEN}✓ Response saved to ${OUTPUT_DIR}/3-list-live-inputs.json${NC}\n"

# Analysis of responses
echo -e "${BLUE}=== Initial Analysis ===${NC}\n"

echo "Checking for HLS playback URL in Live Input response..."
HLS_URL=$(echo "$GET_RESPONSE" | jq -r '.result.playback.hls // .result.hls // empty')
if [ -n "$HLS_URL" ]; then
    echo -e "${GREEN}✓ Found HLS URL: ${HLS_URL}${NC}"
else
    echo -e "${YELLOW}⚠ No HLS URL found in Live Input response${NC}"
fi

echo -e "\nChecking for 'created' field with Video asset..."
CREATED_UID=$(echo "$GET_RESPONSE" | jq -r '.result.created.uid // empty')
if [ -n "$CREATED_UID" ]; then
    echo -e "${GREEN}✓ Found created.uid (Video asset): ${CREATED_UID}${NC}"
else
    echo -e "${YELLOW}⚠ No created.uid field found${NC}"
fi

echo -e "\nChecking stream status..."
STATUS=$(echo "$GET_RESPONSE" | jq -r '.result.status')
echo -e "Status: ${STATUS}"

echo -e "\nChecking for playback URLs..."
WEBRTC_PLAYBACK=$(echo "$GET_RESPONSE" | jq -r '.result.webRTCPlayback.url // empty')
if [ -n "$WEBRTC_PLAYBACK" ]; then
    echo -e "WebRTC Playback: ${WEBRTC_PLAYBACK}"
fi

# Optional: Test with active streaming
echo -e "\n${YELLOW}=== Optional: Active Streaming Test ===${NC}"
echo "To test with actual streaming, run this command in another terminal:"
echo -e "${BLUE}"
echo "ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \\"
echo "  -f lavfi -i sine=frequency=1000:sample_rate=44100 \\"
echo "  -c:v libx264 -preset veryfast -tune zerolatency \\"
echo "  -c:a aac -ar 44100 \\"
echo "  -f flv ${RTMP_URL}${STREAM_KEY}"
echo -e "${NC}"
echo -e "Then press ENTER to check the Live Input again (or Ctrl+C to skip)..."
read -r

# Test 4: Get Live Input Details (During/After Streaming)
echo -e "\n${YELLOW}=== Test 4: Get Live Input Details (During/After Streaming) ===${NC}"

GET_RESPONSE_ACTIVE=$(curl -s \
  "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}" \
  -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}")

echo "$GET_RESPONSE_ACTIVE" | jq '.' > "${OUTPUT_DIR}/4-get-live-input-while-streaming.json"
echo -e "${GREEN}✓ Response saved to ${OUTPUT_DIR}/4-get-live-input-while-streaming.json${NC}\n"

# Analysis after streaming
echo -e "${BLUE}=== Analysis After Streaming ===${NC}\n"

echo "Checking for HLS playback URL..."
HLS_URL_ACTIVE=$(echo "$GET_RESPONSE_ACTIVE" | jq -r '.result.playback.hls // .result.hls // empty')
if [ -n "$HLS_URL_ACTIVE" ]; then
    echo -e "${GREEN}✓ Found HLS URL: ${HLS_URL_ACTIVE}${NC}"
else
    echo -e "${YELLOW}⚠ Still no HLS URL found${NC}"
fi

echo -e "\nChecking for 'created' field with Video asset..."
CREATED_UID_ACTIVE=$(echo "$GET_RESPONSE_ACTIVE" | jq -r '.result.created.uid // empty')
if [ -n "$CREATED_UID_ACTIVE" ]; then
    echo -e "${GREEN}✓ Found created.uid (Video asset): ${CREATED_UID_ACTIVE}${NC}"
    
    # If we found a video UID, try to get video details
    echo -e "\n${YELLOW}=== Test 5: Get Video Asset Details ===${NC}"
    VIDEO_RESPONSE=$(curl -s \
      "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/${CREATED_UID_ACTIVE}" \
      -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}")
    
    echo "$VIDEO_RESPONSE" | jq '.' > "${OUTPUT_DIR}/5-get-video-asset.json"
    echo -e "${GREEN}✓ Response saved to ${OUTPUT_DIR}/5-get-video-asset.json${NC}\n"
    
    # Check for HLS in video response
    VIDEO_HLS=$(echo "$VIDEO_RESPONSE" | jq -r '.result.playback.hls // empty')
    if [ -n "$VIDEO_HLS" ]; then
        echo -e "${GREEN}✓ Found HLS URL in Video asset: ${VIDEO_HLS}${NC}"
    fi
else
    echo -e "${YELLOW}⚠ Still no created.uid field found${NC}"
fi

echo -e "\nUpdated status:"
STATUS_ACTIVE=$(echo "$GET_RESPONSE_ACTIVE" | jq -r '.result.status')
echo -e "Status: ${STATUS_ACTIVE}"

# Summary
echo -e "\n${BLUE}=== Discovery Summary ===${NC}\n"
echo "All responses saved to: ${OUTPUT_DIR}/"
echo -e "\nKey Findings:"
echo "1. Live Input UID: ${LIVE_INPUT_UID}"
echo "2. HLS URL before streaming: ${HLS_URL:-'NOT FOUND'}"
echo "3. HLS URL after streaming: ${HLS_URL_ACTIVE:-'NOT FOUND'}"
echo "4. Video asset UID: ${CREATED_UID_ACTIVE:-'NOT FOUND'}"
echo -e "\n${YELLOW}Review the JSON files in ${OUTPUT_DIR}/ for complete API structure${NC}"

# Cleanup prompt
echo -e "\n${YELLOW}=== Cleanup ===${NC}"
echo "Do you want to delete the test Live Input? (y/n)"
read -r DELETE_RESPONSE

if [ "$DELETE_RESPONSE" = "y" ]; then
    echo "Deleting Live Input..."
    DELETE_RESULT=$(curl -s -X DELETE \
      "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}" \
      -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}")
    
    echo "$DELETE_RESULT" | jq '.' > "${OUTPUT_DIR}/6-delete-live-input.json"
    
    if [ "$(echo "$DELETE_RESULT" | jq -r '.success')" = "true" ]; then
        echo -e "${GREEN}✓ Live Input deleted${NC}"
    else
        echo -e "${RED}ERROR: Failed to delete Live Input${NC}"
        echo "$DELETE_RESULT" | jq '.'
    fi
else
    echo -e "${YELLOW}⚠ Live Input not deleted. Remember to clean up manually!${NC}"
    echo "To delete later, run:"
    echo "curl -X DELETE 'https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}' -H 'Authorization: Bearer ${CLOUDFLARE_API_TOKEN}'"
fi

echo -e "\n${GREEN}=== API Discovery Complete ===${NC}"
