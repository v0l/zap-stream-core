#!/bin/bash

# ==========================================
# RML RTMP Custom Keys Test
# ==========================================
# 
# Tests whether the /api/v1/keys endpoint works
# with the original RML RTMP backend.

set -e  # Exit on error

# Test credentials (safe test keypair, not production)
TEST_NSEC="nsec107gexedhvf97ej83jzalley9wt682mlgy9ty5xwsp98vnph09fysssnzlk"
TEST_NPUB="npub1u0mm82x7muct7cy8y7urztyctgm0r6k27gdax04fa4q28x7q0shq6slmah"

echo "========================================"
echo "RML RTMP Custom Keys Test"
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

if ! command -v jq &> /dev/null; then
    echo "❌ ERROR: jq not found"
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

# Verify backend configuration
echo "[Config] Checking backend type..."
BACKEND_TYPE=$(docker exec zap-stream-core-core-1 cat /app/config.yaml | grep 'backend:' | awk '{print $2}' | tr -d '"' | tr -d "'")
echo "Backend configured as: $BACKEND_TYPE"

if [ "$BACKEND_TYPE" != "rml_rtmp" ]; then
    echo "⚠️  WARNING: Backend is not rml_rtmp, test may not be valid"
fi
echo ""

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Decode npub to hex
TEST_PUBKEY_HEX=$(node "$SCRIPT_DIR/decode_npub.js" "$TEST_NPUB" 2>&1)
if [ $? -ne 0 ]; then
    echo "❌ Failed to decode npub"
    exit 1
fi

echo "Test pubkey hex: $TEST_PUBKEY_HEX"

# Ensure user exists in database
docker exec zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "INSERT IGNORE INTO user (pubkey, balance) VALUES (UNHEX('${TEST_PUBKEY_HEX}'), 0);" \
  2>/dev/null || true

echo ""
echo "========================================"
echo "TEST 1: Create Custom Key (RML RTMP)"
echo "========================================"
echo ""
echo "This is the CRITICAL test that failed with Cloudflare backend."
echo "Testing if the database foreign key constraint error exists upstream."
echo ""

# Create NIP-98 auth for POST
POST_URL="http://localhost:80/api/v1/keys"
POST_AUTH_JSON=$(node "$SCRIPT_DIR/sign_nip98.js" "$TEST_NSEC" "$POST_URL" "POST" 2>&1)
if [ $? -ne 0 ]; then
    echo "❌ Failed to create NIP-98 auth for POST"
    exit 1
fi
POST_AUTH_TOKEN=$(echo "$POST_AUTH_JSON" | base64)

# Create custom key with metadata
CUSTOM_KEY_REQUEST='{
  "event": {
    "title": "RML RTMP Test Stream",
    "summary": "Testing if custom keys work with RML RTMP backend",
    "tags": ["test", "rml-rtmp"]
  }
}'

echo "Creating custom key..."
CREATE_RESPONSE=$(curl -s -X POST "$POST_URL" \
  -H "Authorization: Nostr $POST_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$CUSTOM_KEY_REQUEST")

echo "Response received:"
echo "$CREATE_RESPONSE" | jq '.' 2>/dev/null || echo "$CREATE_RESPONSE"
echo ""

# Check if we got a key back
if echo "$CREATE_RESPONSE" | jq -e '.key' > /dev/null 2>&1; then
    CUSTOM_KEY=$(echo "$CREATE_RESPONSE" | jq -r '.key')
    echo "✅ SUCCESS: Custom key created: $CUSTOM_KEY"
    echo "✓ Key length: ${#CUSTOM_KEY} characters"
    
    # Validate UUID format
    if [[ $CUSTOM_KEY =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]; then
        echo "✓ Format: Valid UUID (RML RTMP format)"
    else
        echo "⚠️  WARNING: Unexpected format: $CUSTOM_KEY"
    fi
    
    echo ""
    echo "========================================"
    echo "TEST 2: List Custom Keys"
    echo "========================================"
    
    # Create auth for GET
    GET_KEYS_URL="http://localhost:80/api/v1/keys"
    GET_AUTH_JSON=$(node "$SCRIPT_DIR/sign_nip98.js" "$TEST_NSEC" "$GET_KEYS_URL" "GET" 2>&1)
    if [ $? -ne 0 ]; then
        echo "❌ Failed to create NIP-98 auth for GET"
        exit 1
    fi
    GET_AUTH_TOKEN=$(echo "$GET_AUTH_JSON" | base64)
    
    echo "Listing all custom keys..."
    KEYS_LIST=$(curl -s "$GET_KEYS_URL" -H "Authorization: Nostr $GET_AUTH_TOKEN")
    
    echo "Keys list:"
    echo "$KEYS_LIST" | jq '.' 2>/dev/null || echo "$KEYS_LIST"
    echo ""
    
    if echo "$KEYS_LIST" | jq -e '.[0]' > /dev/null 2>&1; then
        # Find our key in the list
        KEY_FOUND=$(echo "$KEYS_LIST" | jq --arg key "$CUSTOM_KEY" '.[] | select(.key == $key)')
        
        if [ -n "$KEY_FOUND" ]; then
            echo "✅ SUCCESS: Custom key found in list"
            STREAM_ID=$(echo "$KEY_FOUND" | jq -r '.stream_id')
            echo "✓ Associated stream_id: $STREAM_ID"
        else
            echo "❌ FAILED: Custom key not found in list"
            exit 1
        fi
    else
        echo "❌ FAILED: Could not list keys"
        exit 1
    fi
    
    echo ""
    echo "========================================"
    echo "CONCLUSION"
    echo "========================================"
    echo ""
    echo "✅ The /api/v1/keys endpoint WORKS with RML RTMP backend!"
    echo ""
    echo "This means:"
    echo "  • The bug does NOT exist in upstream code"
    echo "  • The previous AI's change introduced the bug"
    echo "  • The database foreign key constraint is NOT the issue"
    echo "  • The actual problem is likely in how Cloudflare backend"
    echo "    generates keys or handles the stream creation flow"
    echo ""
    
else
    # Check for specific error messages
    ERROR_MSG=$(echo "$CREATE_RESPONSE" | jq -r '.error // empty' 2>/dev/null)
    if [ -z "$ERROR_MSG" ]; then
        ERROR_MSG="$CREATE_RESPONSE"
    fi
    
    echo "❌ FAILED: Could not create custom key"
    echo "Error: $ERROR_MSG"
    echo ""
    
    # Check if it's the foreign key constraint error
    if echo "$ERROR_MSG" | grep -q "foreign key constraint"; then
        echo "========================================"
        echo "CONCLUSION"
        echo "========================================"
        echo ""
        echo "⚠️  FOREIGN KEY CONSTRAINT ERROR DETECTED"
        echo ""
        echo "This means:"
        echo "  • The bug EXISTS in upstream code"
        echo "  • The /api/v1/keys endpoint was never working/tested"
        echo "  • The database schema has a fundamental issue"
        echo "  • Need to fix the order of operations in create_stream_key()"
        echo ""
    fi
    
    exit 1
fi

echo ""
echo "========================================"
echo "TEST SUMMARY"
echo "========================================"
echo "✅ TEST 1: Create Custom Key - PASSED"
echo "✅ TEST 2: List Custom Keys - PASSED"
echo ""
echo "All tests passed with RML RTMP backend!"
