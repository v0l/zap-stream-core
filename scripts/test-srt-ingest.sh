#!/usr/bin/env bash
set -euo pipefail

# Cloudflare Stream SRT Ingest Test
# Usage:
#   ./scripts/test-srt-ingest.sh                          # interactive prompts
#   CF_TOKEN=xxx CF_ACCOUNT=yyy ./scripts/test-srt-ingest.sh                 # list inputs, pick one
#   CF_TOKEN=xxx CF_ACCOUNT=yyy CF_INPUT_UID=zzz ./scripts/test-srt-ingest.sh  # use specific input

CF_API="https://api.cloudflare.com/client/v4"

# --- Collect credentials ---
if [[ -z "${CF_TOKEN:-}" ]]; then
  read -rp "Cloudflare API Token: " CF_TOKEN
fi
if [[ -z "${CF_ACCOUNT:-}" ]]; then
  read -rp "Cloudflare Account ID: " CF_ACCOUNT
fi

auth_header="Authorization: Bearer $CF_TOKEN"

# --- Helper: pretty-print JSON if jq is available ---
pp() { if command -v jq &>/dev/null; then jq .; else cat; fi; }

echo ""
echo "=== Step 1: Resolve Live Input ==="

if [[ -n "${CF_INPUT_UID:-}" ]]; then
  echo "Using provided Live Input UID: $CF_INPUT_UID"
else
  echo "Listing Live Inputs..."
  inputs_json=$(curl -sf -H "$auth_header" \
    "$CF_API/accounts/$CF_ACCOUNT/stream/live_inputs?per_page=10")

  echo "$inputs_json" | jq -r '.result[] | "\(.uid)  \(.meta.name // "unnamed")  created=\(.created)"' 2>/dev/null || {
    echo "Raw response:"
    echo "$inputs_json" | pp
  }

  echo ""
  read -rp "Enter Live Input UID to use (or press enter to create a new one): " CF_INPUT_UID

  if [[ -z "$CF_INPUT_UID" ]]; then
    echo "Creating new Live Input for SRT test..."
    create_json=$(curl -sf -X POST \
      -H "$auth_header" \
      -H "Content-Type: application/json" \
      -d '{"meta":{"name":"srt-ingest-test"},"recording":{"mode":"automatic"}}' \
      "$CF_API/accounts/$CF_ACCOUNT/stream/live_inputs")

    CF_INPUT_UID=$(echo "$create_json" | jq -r '.result.uid')
    echo "Created Live Input: $CF_INPUT_UID"
    echo "$create_json" | pp
  fi
fi

echo ""
echo "=== Step 2: Fetch SRT endpoint details ==="

input_json=$(curl -sf -H "$auth_header" \
  "$CF_API/accounts/$CF_ACCOUNT/stream/live_inputs/$CF_INPUT_UID")

echo "Full Live Input response:"
echo "$input_json" | pp

# Extract SRT details
srt_url=$(echo "$input_json" | jq -r '.result.srt.url // empty')
srt_stream_id=$(echo "$input_json" | jq -r '.result.srt.streamId // empty')
srt_passphrase=$(echo "$input_json" | jq -r '.result.srt.passphrase // empty')

# Extract RTMPS for comparison
rtmps_url=$(echo "$input_json" | jq -r '.result.rtmps.url // empty')
rtmps_key=$(echo "$input_json" | jq -r '.result.rtmps.streamKey // empty')

echo ""
echo "--- RTMPS Endpoint ---"
echo "  URL:        $rtmps_url"
echo "  Stream Key: $rtmps_key"
echo ""
echo "--- SRT Endpoint ---"
echo "  URL:        ${srt_url:-NOT AVAILABLE}"
echo "  Stream ID:  ${srt_stream_id:-NOT AVAILABLE}"
echo "  Passphrase: ${srt_passphrase:-NOT AVAILABLE}"

if [[ -z "$srt_url" ]]; then
  echo ""
  echo "WARNING: SRT endpoint not returned by Cloudflare for this Live Input."
  echo "This may mean SRT is not enabled on your account or this input."
  echo ""
  echo "You can still test with RTMPS. Run:"
  echo "  ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \\"
  echo "    -f lavfi -i sine=frequency=1000:sample_rate=48000 \\"
  echo "    -c:v libx264 -b:v 2000k -g 60 -c:a aac -b:a 128k \\"
  echo "    -f flv '${rtmps_url}${rtmps_key}'"
  exit 1
fi

echo ""
echo "=== Step 3: Generated ffmpeg commands ==="

# Build the full SRT URL with query params
full_srt_url="${srt_url}?streamid=${srt_stream_id}&passphrase=${srt_passphrase}"

echo ""
echo "--- Option A: SRT ingest (test pattern, 30 seconds) ---"
echo ""
SRT_CMD="ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=48000 \
  -c:v libx264 -preset veryfast -b:v 2000k -g 60 \
  -c:a aac -b:a 128k \
  -f mpegts '${full_srt_url}'"
echo "$SRT_CMD"

echo ""
echo "--- Option B: RTMPS ingest (same test pattern, for comparison) ---"
echo ""
RTMPS_CMD="ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=48000 \
  -c:v libx264 -preset veryfast -b:v 2000k -g 60 \
  -c:a aac -b:a 128k \
  -f flv '${rtmps_url}${rtmps_key}'"
echo "$RTMPS_CMD"

echo ""
echo "=== Step 4: Start SRT stream? ==="
read -rp "Stream via SRT now? (y/N): " do_stream

if [[ "${do_stream,,}" == "y" ]]; then
  DURATION="${STREAM_DURATION:-30}"
  echo "Streaming test pattern via SRT for ${DURATION}s..."
  echo "(Press Ctrl+C to stop early)"
  echo ""

  ffmpeg -re -f lavfi -i "testsrc=size=1280x720:rate=30" \
    -f lavfi -i "sine=frequency=1000:sample_rate=48000" \
    -c:v libx264 -preset veryfast -b:v 2000k -g 60 \
    -c:a aac -b:a 128k \
    -t "$DURATION" \
    -f mpegts "${full_srt_url}"

  echo ""
  echo "Stream finished. Checking status..."
  sleep 2

  status_json=$(curl -sf -H "$auth_header" \
    "$CF_API/accounts/$CF_ACCOUNT/stream/live_inputs/$CF_INPUT_UID")
  status=$(echo "$status_json" | jq -r '.result.status // "unknown"')
  echo "Live Input status after stream: $status"
else
  echo ""
  echo "Copy and run the SRT command above to test manually."
fi

echo ""
echo "=== Done ==="
