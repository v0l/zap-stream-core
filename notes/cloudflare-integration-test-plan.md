# Cloudflare Integration Test Plan

## Current Status

### What Works ✓
- Rust unit tests pass (confirmed)
- Docker RML RTMP integration tests pass (confirmed)
- CloudflareBackend code is implemented
- Config is set to `backend: "cloudflare"`
- Test script exists at `scripts/test-cloudflare-e2e.sh`
- Docker port mapping: host port 80 → container port 8080

### The Problem ✗
The `public_url` in `compose-config.local.yaml` points to an OLD tunnel:
```yaml
public_url: "https://memphis-surveillance-day-reverse.trycloudflare.com"
```

This tunnel is not running, so:
- Cloudflare cannot send webhooks to your local server
- Previous AI tests showed "⚠ Warning: Did not find webhook" 
- Integration test cannot prove end-to-end functionality

## The Solution

### Step 1: Start Cloudflare Quick Tunnel

Run this command on your HOST machine (NOT in Docker):
```bash
cloudflared tunnel --url http://localhost:80
```

This will output something like:
```
Your quick Tunnel has been created! Visit it at:
https://random-words-xyz.trycloudflare.com
```

**Copy that URL - you'll need it for Step 2!**

Keep this terminal window open - the tunnel must stay running during testing.

### Step 2: Configure Cloudflare Dashboard Webhook

**IMPORTANT**: The notification policy is ONE-TIME setup, but the webhook URL must be updated each time you create a new tunnel.

1. **Log into Cloudflare Dashboard**:
   - Go to https://dash.cloudflare.com
   - Select your account

2. **Create or Update Webhook Destination**:
   - Navigate to **Notifications** → **Destinations** tab
   - Under **Webhooks**:
     - If `shosho-stream-live` webhook exists: Click **Edit** → Update URL
     - If not: Click **Create** → Enter details below
   - Fill in:
     - **Name**: `shosho-stream-live`
     - **URL**: `https://YOUR-TUNNEL-URL.trycloudflare.com/webhooks/cloudflare`
       - (Paste the tunnel URL from Step 1)
     - **Secret**: (leave blank for now, optional)
   - Click **Save and Test**

3. **Create Stream Live Notification Policy** (ONLY if not already created):
   - Navigate to **Notifications** → **All Notifications** tab
   - If you already see `shosho-live-input-events`, skip to Step 3
   - Otherwise, click **Add** button
   - Under **Product** section, find and select **Stream**
   - Select **Stream Live Input** notification type
   - Fill in:
     - **Name**: `shosho-live-input-events`
     - **Description**: (optional)
   - Under **Webhooks**, click **Add webhook**
   - Select your `shosho-stream-live` webhook
   - Click **Next**
   - **Leave "Stream Live IDs" field BLANK** (this makes it apply to ALL inputs)
   - Ensure both event types are checked:
     - ☑ `live_input.connected`
     - ☑ `live_input.disconnected`
   - Click **Create**

**✅ Done!** This configuration now applies to ALL Live Inputs (current and future).

### Step 3: Update Config With New Tunnel URL

Edit `zap-stream-core/docs/deploy/compose-config.local.yaml`:

Change this line:
```yaml
public_url: "https://memphis-surveillance-day-reverse.trycloudflare.com"
```

To your new tunnel URL (from Step 1):
```yaml
public_url: "https://random-words-xyz.trycloudflare.com"
```

### Step 4: Restart Docker

```bash
cd /Users/visitor/Projects/shosho/zap-stream-core/docs/deploy
docker-compose down
docker-compose up --build -d
```

Wait for containers to fully start (~30 seconds).

### Step 5: Verify Docker is Running

```bash
docker ps | grep zap-stream-core
```

Should show 3 running containers:
- `zap-stream-core-core-1`
- `zap-stream-core-db-1`
- `zap-stream-core-redis-1`

### Step 6: Run Integration Test

```bash
cd /Users/visitor/Projects/shosho/zap-stream-core
bash scripts/test-cloudflare-e2e.sh
```

### Step 7: Verify Success Criteria

The test output MUST show ALL of these:
- ✓ API call with NIP-98 auth succeeded
- ✓ Cloudflare RTMP endpoint received
- ✓ FFmpeg streaming started
- ✓ **Webhook received: live_input.connected** (NOT ⚠ warning!)
- ✓ **Stream connected event found** (NOT ⚠ warning!)
- ✓ Video Asset created
- ✓ **Webhook received: live_input.disconnected** (NOT ⚠ warning!)
- ✓ **Stream disconnected event found** (NOT ⚠ warning!)

If you see ⚠ warnings instead of ✓ checkmarks for webhooks, the test FAILED.

## Why This Works

```
Cloudflare RTMP → Cloudflare processes stream
                ↓
        Sends webhook to public_url
                ↓
        cloudflared tunnel (on host:localhost:80)
                ↓
        Docker port mapping (host:80 → container:8080)
                ↓
        zap-stream-core app receives webhook
                ↓
        Parses webhook, updates database, publishes Nostr event
```

## Troubleshooting

### "Webhook not received"
- Verify tunnel is still running (check terminal window)
- Verify public_url in config matches tunnel URL exactly
- Verify Docker was restarted after config change
- Check Docker logs: `docker logs zap-stream-core-core-1`

### "API call failed"
- Verify Docker containers are running: `docker ps`
- Check container logs for errors
- Verify database is accessible: `docker logs zap-stream-core-db-1`

### "FFmpeg died immediately"
- Check Cloudflare Stream API token and account ID in config
- Verify network connectivity
- Check FFmpeg error logs (printed by test script)

## What This Proves

A successful test with webhook verification proves:
1. ✓ Cloudflare Live Input API works
2. ✓ Streaming to Cloudflare RTMP works  
3. ✓ Webhook delivery mechanism works
4. ✓ Webhook parsing and handling works
5. ✓ Database updates work
6. ✓ Complete stream lifecycle (start → live → end) works
7. ✓ **The Cloudflare backend integration is fully functional**
