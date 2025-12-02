#!/bin/bash
# Cloudflare Backend Integration Test Script
# Purpose: Validate that Cloudflare streaming backend works end-to-end

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration (read from compose-config.local.yaml)
CONFIG_FILE="docs/deploy/compose-config.local.yaml"
API_TOKEN=$(grep "api-token:" "$CONFIG_FILE" | awk '{print $2}' | tr -d '"')
ACCOUNT_ID=$(grep "account-id:" "$CONFIG_FILE" | awk '{print $2}' | tr -d '"')
PUBLIC_URL=$(grep "public_url:" "$CONFIG_FILE" | awk '{print $2}' | tr -d '"')

# Test user configuration
TEST_USER_ID=55
TEST_STREAM_KEY="81b97dd0-b959-11f0-b22c-d690ca11bae8"

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Cloudflare Integration Test Suite${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
echo -e "Config: API Token: ${API_TOKEN:0:10}..."
echo -e "Config: Account ID: $ACCOUNT_ID"
echo -e "Config: Public URL: $PUBLIC_URL"
echo ""

# =============================================================================
# LEVEL 1: API CONNECTIVITY TEST
# =============================================================================
echo -e "${YELLOW}[LEVEL 1] Testing Cloudflare API Connectivity${NC}"
echo -e "Purpose: Verify API credentials and connectivity"
echo ""

test_cloudflare_api() {
    echo -e "Test 1.1: Creating Live Input via Cloudflare API..."
    
    LIVE_INPUT_RESPONSE=$(curl -s -X POST \
        "https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs" \
        -H "Authorization: Bearer $API_TOKEN" \
        -H "Content-Type: application/json" \
        -d '{
            "meta": {"name": "Integration Test - '$(date +%s)'"},
            "recording": {"mode": "automatic"}
        }')
    
    if echo "$LIVE_INPUT_RESPONSE" | jq -e '.success == true' > /dev/null 2>&1; then
        echo -e "${GREEN}✓ PASS${NC}: Live Input created successfully"
        
        LIVE_INPUT_UID=$(echo "$LIVE_INPUT_RESPONSE" | jq -r '.result.uid')
        RTMPS_URL=$(echo "$LIVE_INPUT_RESPONSE" | jq -r '.result.rtmps.url')
        RTMPS_KEY=$(echo "$LIVE_INPUT_RESPONSE" | jq -r '.result.rtmps.streamKey')
        
        echo "  Live Input UID: $LIVE_INPUT_UID"
        echo "  RTMPS URL: $RTMPS_URL"
        echo "  Stream Key: ${RTMPS_KEY:0:20}..."
        
        # Clean up test input
        echo ""
        echo "Test 1.2: Cleaning up test Live Input..."
        DELETE_RESPONSE=$(curl -s -X DELETE \
            "https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_UID" \
            -H "Authorization: Bearer $API_TOKEN")
        
        if echo "$DELETE_RESPONSE" | jq -e '.success == true' > /dev/null 2>&1; then
            echo -e "${GREEN}✓ PASS${NC}: Test Live Input deleted"
        else
            echo -e "${YELLOW}⚠ WARNING${NC}: Could not delete test Live Input"
        fi
        
        return 0
    else
        echo -e "${RED}✗ FAIL${NC}: Could not create Live Input"
        echo "Response: $LIVE_INPUT_RESPONSE"
        return 1
    fi
}

if test_cloudflare_api; then
    echo -e "${GREEN}[LEVEL 1] ✓ PASSED${NC}: Cloudflare API is accessible"
else
    echo -e "${RED}[LEVEL 1] ✗ FAILED${NC}: Cloudflare API connectivity issue"
    echo "Check your API credentials in $CONFIG_FILE"
    exit 1
fi

echo ""
echo -e "${BLUE}========================================${NC}"
echo ""

# =============================================================================
# LEVEL 2: WEBHOOK HANDLER TEST
# =============================================================================
echo -e "${YELLOW}[LEVEL 2] Testing Webhook Handler (Local)${NC}"
echo -e "Purpose: Verify webhook processing code works"
echo ""

test_webhook_handler() {
    echo "Test 2.1: Checking if Docker is running..."
    
    if ! docker ps | grep -q "zap-stream-core-core-1"; then
        echo -e "${RED}✗ FAIL${NC}: Docker container 'zap-stream-core-core-1' is not running"
        echo "Please start Docker with: cd docs/deploy && docker-compose up -d"
        return 1
    fi
    
    echo -e "${GREEN}✓${NC} Docker container is running"
    echo ""
    
    echo "Test 2.2: Sending fake webhook (connected event)..."
    
    # Create fake webhook payload
    FAKE_WEBHOOK_CONNECTED='{
        "name": "Live Webhook Test",
        "text": "Notification type: Stream Live Input\nInput ID: test-integration-12345\nEvent type: live_input.connected\nUpdated at: '$(date -u +%Y-%m-%dT%H:%M:%SZ)'",
        "data": {
            "notification_name": "Stream Live Input",
            "input_id": "test-integration-12345",
            "event_type": "live_input.connected",
            "updated_at": "'$(date -u +%Y-%m-%dT%H:%M:%SZ)'"
        },
        "ts": '$(date +%s)'
    }'
    
    WEBHOOK_RESPONSE=$(curl -s -X POST \
        "http://localhost:80/webhooks/cloudflare" \
        -H "Content-Type: application/json" \
        -d "$FAKE_WEBHOOK_CONNECTED")
    
    if [ "$WEBHOOK_RESPONSE" == "OK" ]; then
        echo -e "${GREEN}✓ PASS${NC}: Webhook handler accepted the payload"
    else
        echo -e "${RED}✗ FAIL${NC}: Webhook handler rejected the payload"
        echo "Response: $WEBHOOK_RESPONSE"
        return 1
    fi
    
    echo ""
    echo "Test 2.3: Checking Docker logs for webhook processing..."
    sleep 2
    
    RECENT_LOGS=$(docker logs --tail 50 zap-stream-core-core-1 2>&1)
    
    if echo "$RECENT_LOGS" | grep -q "Received webhook for backend: cloudflare"; then
        echo -e "${GREEN}✓${NC} Webhook received by server"
    else
        echo -e "${YELLOW}⚠${NC} Could not confirm webhook receipt in logs"
    fi
    
    if echo "$RECENT_LOGS" | grep -q "Received Cloudflare webhook event"; then
        echo -e "${GREEN}✓${NC} Cloudflare webhook parsed successfully"
    else
        echo -e "${YELLOW}⚠${NC} Webhook parsing not confirmed in logs"
    fi
    
    return 0
}

if test_webhook_handler; then
    echo -e "${GREEN}[LEVEL 2] ✓ PASSED${NC}: Webhook handler is functional"
else
    echo -e "${RED}[LEVEL 2] ✗ FAILED${NC}: Webhook handler has issues"
    echo "Check Docker logs: docker logs --tail 100 zap-stream-core-core-1"
    exit 1
fi

echo ""
echo -e "${BLUE}========================================${NC}"
echo ""

# =============================================================================
# LEVEL 3: END-TO-END STREAMING TEST
# =============================================================================
echo -e "${YELLOW}[LEVEL 3] End-to-End Streaming Test${NC}"
echo -e "Purpose: Test actual streaming through Cloudflare"
echo ""

echo -e "${BLUE}This test requires manual steps:${NC}"
echo ""
echo "1. Verify your cloudflare tunnel is running:"
echo "   Check if $PUBLIC_URL is accessible"
echo ""
echo "2. Get the RTMP streaming URL:"
echo "   The system should have created Live Inputs during startup."
echo "   Check Docker logs for RTMP URLs:"
echo "   ${BLUE}docker logs zap-stream-core-core-1 | grep 'Created Live Input UID'${NC}"
echo ""
echo "3. Stream test pattern to Cloudflare:"
echo '   ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \'
echo '     -f lavfi -i sine=frequency=1000:sample_rate=44100 \'
echo '     -c:v libx264 -preset veryfast -tune zerolatency \'
echo '     -c:a aac -ar 44100 \'
echo '     -f flv rtmps://live.cloudflare.com:443/live/{YOUR-STREAM-KEY}'
echo ""
echo "4. Monitor webhook reception:"
echo "   ${BLUE}docker logs -f zap-stream-core-core-1 | grep -i 'webhook\|cloudflare'${NC}"
echo ""
echo "5. Check for successful stream start:"
echo "   Look for these log messages:"
echo "   - 'Received Cloudflare webhook event: live_input.connected'"
echo "   - 'Stream connected: {stream-id}'"
echo "   - 'Video Asset found! UID: ...'"
echo ""
echo "6. Verify HLS playback:"
echo "   The HLS URL should be available from the API:"
echo "   ${BLUE}curl http://localhost:8080/api/v1/account -H 'Authorization: Nostr <event>'${NC}"
echo ""
echo "7. Stop streaming (press Ctrl+C in ffmpeg)"
echo "   Then verify disconnection webhook:"
echo "   - 'Received Cloudflare webhook event: live_input.disconnected'"
echo "   - 'Stream disconnected: {stream-id}'"
echo ""

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Test Summary${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
echo -e "${GREEN}✓ Level 1 PASSED:${NC} Cloudflare API is accessible"
echo -e "${GREEN}✓ Level 2 PASSED:${NC} Webhook handler works locally"
echo -e "${YELLOW}⧗ Level 3 MANUAL:${NC} Follow instructions above for end-to-end test"
echo ""
echo -e "${BLUE}Next Steps:${NC}"
echo "1. If Level 3 webhooks don't arrive, check:"
echo "   - Is cloudflare tunnel running? ($PUBLIC_URL)"
echo "   - Can Cloudflare reach it? (test from external network)"
echo "   - Are webhooks configured? (check CF dashboard or logs from startup)"
echo ""
echo "2. If streaming works but no HLS URL:"
echo "   - Check 'Video Asset not yet created' in logs"
echo "   - May need to wait 5-10 seconds for Cloudflare to process"
echo ""
echo "3. If everything works:"
echo "   - Congratulations! Cloudflare backend is functional! 🎉"
echo ""
