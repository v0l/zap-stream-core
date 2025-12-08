#!/bin/bash

# ==========================================
# Cloudflare Viewer Count API Test
# ==========================================
# 
# This script tests the Cloudflare Stream Analytics API
# to verify the correct endpoint for viewer counts.

set -e  # Exit on error

echo "========================================"
echo "Cloudflare Viewer Count API Test"
echo "========================================"
echo ""

# Check prerequisites
if ! command -v jq &> /dev/null; then
    echo "❌ ERROR: jq not found (required for JSON parsing)"
    echo "Install with: brew install jq"
    exit 1
fi

if ! command -v yq &> /dev/null; then
    echo "⚠️  WARNING: yq not found (needed to parse YAML config)"
    echo "Will use fallback method to read config"
fi

# Get config path
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_PATH="$SCRIPT_DIR/../docs/deploy/compose-config.local.yaml"

if [ ! -f "$CONFIG_PATH" ]; then
    echo "❌ ERROR: Config file not found: $CONFIG_PATH"
    exit 1
fi

echo "[1/5] Reading Cloudflare credentials from config..."

# Extract API token and account ID from YAML config
# Using grep/sed as a fallback if yq is not available
API_TOKEN=$(grep "api-token:" "$CONFIG_PATH" | sed 's/.*api-token: *"\([^"]*\)".*/\1/' | tr -d ' ')
ACCOUNT_ID=$(grep "account-id:" "$CONFIG_PATH" | sed 's/.*account-id: *"\([^"]*\)".*/\1/' | tr -d ' ')

if [ -z "$API_TOKEN" ] || [ -z "$ACCOUNT_ID" ]; then
    echo "❌ ERROR: Could not extract API credentials from config"
    exit 1
fi

echo "✓ API Token: ${API_TOKEN:0:20}... (${#API_TOKEN} chars)"
echo "✓ Account ID: $ACCOUNT_ID"
echo ""

# Get Live Input UID (from argument or database)
if [ -n "$1" ]; then
    LIVE_INPUT_UID="$1"
    echo "[2/5] Using provided Live Input UID: $LIVE_INPUT_UID"
else
    echo "[2/5] Looking for Live Input UID in database..."
    
    if ! docker ps | grep -q zap-stream-core-db-1; then
        echo "❌ ERROR: Database container not running"
        echo "Usage: $0 <live_input_uid>"
        exit 1
    fi
    
    # Try to find any user with a stream_key
    LIVE_INPUT_UID=$(docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream \
        -e "SELECT stream_key FROM user WHERE stream_key IS NOT NULL AND stream_key != '' LIMIT 1;" \
        -s -N 2>/dev/null || echo "")
    
    if [ -z "$LIVE_INPUT_UID" ]; then
        echo "❌ ERROR: No Live Input UID found in database"
        echo "Usage: $0 <live_input_uid>"
        exit 1
    fi
    
    echo "✓ Found Live Input UID in database: $LIVE_INPUT_UID"
fi

# Validate UID format (32 lowercase hex chars)
if [[ ! $LIVE_INPUT_UID =~ ^[0-9a-f]{32}$ ]]; then
    echo "⚠️  WARNING: UID format may be invalid (expected 32 hex chars)"
fi

echo ""

# Test the official Cloudflare API endpoint
echo "[3/5] Testing Cloudflare Stream Analytics API..."
API_URL="https://api.cloudflare.com/client/v4/accounts/${ACCOUNT_ID}/stream/live_inputs/${LIVE_INPUT_UID}/stats"

echo "Endpoint: $API_URL"
echo ""

echo "[4/5] Making API request..."

# Make the request and capture both response and HTTP status
HTTP_RESPONSE=$(curl -s -w "\n%{http_code}" \
    -H "Authorization: Bearer ${API_TOKEN}" \
    -H "Content-Type: application/json" \
    "$API_URL")

# Extract HTTP status code (last line)
HTTP_STATUS=$(echo "$HTTP_RESPONSE" | tail -n1)

# Extract response body (everything except last line)
RESPONSE_BODY=$(echo "$HTTP_RESPONSE" | sed '$d')

echo ""
echo "[5/5] Response Analysis:"
echo "========================================"
echo "HTTP Status: $HTTP_STATUS"
echo ""

if [ "$HTTP_STATUS" = "200" ]; then
    echo "✅ SUCCESS: API endpoint works!"
    echo ""
    echo "Raw Response:"
    echo "$RESPONSE_BODY" | jq '.' 2>/dev/null || echo "$RESPONSE_BODY"
    echo ""
    
    # Try to extract viewer count
    if echo "$RESPONSE_BODY" | jq -e '.success' >/dev/null 2>&1; then
        SUCCESS=$(echo "$RESPONSE_BODY" | jq -r '.success')
        echo "API Success Field: $SUCCESS"
        
        if [ "$SUCCESS" = "true" ]; then
            # Try to extract viewer count from various possible paths
            VIEWERS=$(echo "$RESPONSE_BODY" | jq -r '.result.live.viewers // .result.viewers // "not_found"' 2>/dev/null)
            
            if [ "$VIEWERS" = "not_found" ]; then
                echo "⚠️  Viewer count field not found in expected location"
                echo "Full result structure:"
                echo "$RESPONSE_BODY" | jq '.result' 2>/dev/null || echo "Could not parse result"
            else
                echo "🎯 VIEWER COUNT: $VIEWERS"
            fi
        fi
    fi
    
    echo ""
    echo "✅ PROOF OF CONCEPT SUCCESSFUL"
    echo "This endpoint can be used for viewer count implementation"
    
elif [ "$HTTP_STATUS" = "404" ]; then
    echo "❌ FAILED: Endpoint not found (404)"
    echo ""
    echo "This could mean:"
    echo "1. The Live Input UID doesn't exist"
    echo "2. The endpoint path is incorrect"
    echo "3. The Live Input was deleted"
    echo ""
    echo "Response:"
    echo "$RESPONSE_BODY" | jq '.' 2>/dev/null || echo "$RESPONSE_BODY"
    
elif [ "$HTTP_STATUS" = "401" ] || [ "$HTTP_STATUS" = "403" ]; then
    echo "❌ FAILED: Authentication error ($HTTP_STATUS)"
    echo ""
    echo "Response:"
    echo "$RESPONSE_BODY" | jq '.' 2>/dev/null || echo "$RESPONSE_BODY"
    
else
    echo "⚠️  UNEXPECTED: HTTP $HTTP_STATUS"
    echo ""
    echo "Response:"
    echo "$RESPONSE_BODY" | jq '.' 2>/dev/null || echo "$RESPONSE_BODY"
fi

echo ""
echo "========================================"
echo "Test complete"
echo "========================================"
