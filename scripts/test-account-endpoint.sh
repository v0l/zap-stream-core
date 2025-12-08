#!/bin/bash

# ==========================================
# Test /api/v1/account Endpoint
# ==========================================
# Simple script to test the account endpoint and view response

set -e  # Exit on error

# Test credentials (safe test keypair)
TEST_NSEC="nsec194nzvgze9xn3df5tmyewh3hs4r0qymcym0jvnjpzg99q897mk82se2r30l"
TEST_NPUB="npub189c0h3jrf8t5z7ngpe8xyl60e25uj4kzw53eu96pf4hg8y7g9crsxer99w"

echo "========================================"
echo "Testing /api/v1/account Endpoint"
echo "========================================"
echo ""
echo "Test User: $TEST_NPUB"
echo ""

# Check prerequisites
if ! command -v node &> /dev/null; then
    echo "❌ ERROR: node not found"
    exit 1
fi

if ! command -v jq &> /dev/null; then
    echo "❌ ERROR: jq not found (install with: brew install jq)"
    exit 1
fi

# Check if Docker container is running
if ! docker ps | grep -q zap-stream-core-core-1; then
    echo "❌ ERROR: zap-stream-core-core-1 container not running"
    echo "   Start it with: cd docs/deploy && docker-compose up -d"
    exit 1
fi

echo "✓ All prerequisites met"
echo ""

# Prepare API call
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
API_URL="http://localhost:80/api/v1/account"

echo "========================================"
echo "Creating NIP-98 Authentication..."
echo "========================================"

# Create NIP-98 auth
AUTH_EVENT_JSON=$(node "$SCRIPT_DIR/sign_nip98.js" "$TEST_NSEC" "$API_URL" "GET" 2>&1)

if [ $? -ne 0 ]; then
    echo "❌ Failed to create NIP-98 auth"
    echo "$AUTH_EVENT_JSON"
    exit 1
fi

AUTH_TOKEN=$(echo "$AUTH_EVENT_JSON" | base64)
echo "✓ Auth token created"
echo ""

echo "========================================"
echo "Calling API: GET $API_URL"
echo "========================================"
echo ""

# Call API
API_RESPONSE=$(curl -s "$API_URL" -H "Authorization: Nostr $AUTH_TOKEN")

# Check if response is valid JSON
if ! echo "$API_RESPONSE" | jq . > /dev/null 2>&1; then
    echo "❌ Invalid JSON response:"
    echo "$API_RESPONSE"
    exit 1
fi

# Pretty print response
echo "$API_RESPONSE" | jq '.'

echo ""
echo "========================================"
echo "Response Summary"
echo "========================================"

# Extract key information
ENDPOINT_COUNT=$(echo "$API_RESPONSE" | jq '.endpoints | length')
BALANCE=$(echo "$API_RESPONSE" | jq '.balance')
HAS_NWC=$(echo "$API_RESPONSE" | jq '.has_nwc')

echo "• Endpoints available: $ENDPOINT_COUNT"
echo "• Balance: $BALANCE sats"
echo "• Has NWC configured: $HAS_NWC"

if [ "$ENDPOINT_COUNT" -gt 0 ]; then
    echo ""
    echo "Endpoints:"
    echo "$API_RESPONSE" | jq -r '.endpoints[] | "  - \(.name): \(.cost.rate) sats/\(.cost.unit) (capabilities: \(.capabilities | join(", ")))"'
fi

echo ""
echo "✅ Test complete!"
