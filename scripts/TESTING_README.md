# Testing Playbook for zap-stream-external

This document covers the full testing workflow for `zap-stream-external`, from unit tests through to manual smoke testing. Follow the steps in order.

## Architecture

The external backend (`zap-stream-external`) replaces local RTMP ingestion with Cloudflare Stream. It exposes an HTTP API on a configurable port (default 8080), processes Cloudflare webhooks for stream lifecycle events, and publishes Nostr kind 30311 events to relay(s).

The test harness validates the full lifecycle: API authentication, Cloudflare Live Input provisioning, RTMPS/SRT streaming, webhook processing, Nostr event publishing, and custom stream key management.

```
┌──────────────┐     NIP-98 auth      ┌─────────────────────────┐
│  Test Script  │ ──────────────────── │  zap-stream-external    │
│  (bash+node)  │     curl requests    │  (Docker container)     │
│               │ ◄─────────────────── │                         │
│               │     JSON responses   │  Port: $ZS_API_PORT     │
└──────┬───────┘                       └────────┬────────────────┘
       │                                        │
       │  FFmpeg RTMPS ──────────────────── Cloudflare Stream
       │                                        │
       │                                   Webhooks (connected/
       │                                    disconnected)
       │                                        │
       │  Nostr query ─────────────────── Nostr Relay ($NOSTR_RELAY_URL)
       │                                        │
       │  MariaDB query ──────────────── MariaDB ($ZS_DB_CONTAINER)
       └────────────────────────────────────────┘
```

## Prerequisites

- **Node.js** (for NIP-98 signing and Nostr relay queries)
- **jq** (JSON parsing)
- **ffmpeg** (test stream generation)
- **Docker** (container access for logs and DB queries)
- **Rust/Cargo** (for running unit tests)
- **Running external stack**: `zap-stream-external` and `db` containers must be up
- **Cloudflare credentials**: Real Cloudflare account with Stream enabled (configured in the external service's `config.yaml`)
- **Nostr relay**: A relay accessible at `$NOSTR_RELAY_URL` that the external service publishes to
- **Webhook reachability**: The external service's `public_url` must be reachable from Cloudflare's servers (use cloudflared tunnel or similar for local dev)

## Quick Reference

| Step | Action | Who |
|------|--------|-----|
| 1 | Run cargo tests | AI Agent |
| 2 | Check cloudflared tunnel liveness | AI Agent |
| 3 | Update docker override with tunnel URL | AI Agent |
| 4 | Restart docker stack | AI Agent |
| 5 | Update Cloudflare webhook notification URL | Developer (in CF dashboard) |
| 6 | Run e2e test scripts | AI Agent |
| 7 | Manual smoke test | Developer |

---

## Step 1: Run Cargo Tests

Before anything else, confirm the code compiles and all unit tests pass.

```bash
cargo test -p zap-stream-external
```

**Expected result:** All tests pass (0 failed). Pre-existing warnings are OK. If any tests fail, fix them before proceeding — there is no point running e2e tests against broken code.

---

## Step 2: Check Cloudflared Tunnel Liveness

The e2e and smoke tests require Cloudflare webhooks to reach your local service. This needs a cloudflared tunnel.

**If a tunnel is already running**, check it's still alive:

```bash
# Find existing cloudflared process
ps aux | grep cloudflared | grep -v grep

# Test the tunnel (should return HTTP 200)
curl -s -o /dev/null -w "%{http_code}" https://<your-tunnel-url>/api/v1/time
```

If the curl returns `200`, the tunnel is live — note the URL and proceed to Step 3.

If the curl returns `000`, times out, or no cloudflared is running, **start a new tunnel**:

```bash
# Kill any stale process
kill <old-pid>

# Start a new quick tunnel
cloudflared tunnel --url http://localhost:8090 &

# cloudflared will print a line like:
#   | https://something-something.trycloudflare.com
# Copy this URL — you need it for Steps 3 and 5
```

Quick tunnel URLs are ephemeral. They change every time you restart cloudflared.

---

## Step 3: Update Docker Override with Tunnel URL

Edit `docs/deploy/docker-compose.override.yml` and set `APP__PUBLIC_URL` to the active tunnel URL from Step 2.

```yaml
environment:
  APP__PUBLIC_URL: "https://<your-tunnel-url>"
```

If the override file does not exist, create one. It must contain:
- `APP__CLOUDFLARE__TOKEN` and `APP__CLOUDFLARE__ACCOUNT_ID` — Cloudflare Stream API credentials
- `APP__PUBLIC_URL` — the tunnel URL (must match the active cloudflared tunnel)
- `APP__NSEC` — Nostr secret key for event signing
- `APP__DATABASE` — MySQL connection string pointing to the `db` service

---

## Step 4: Restart Docker Stack

First check the current state of Docker:

```bash
# See what's running
docker ps --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}' | grep -E '(zap-stream|db|relay)'

# Check logs for errors if the external service is already running
docker logs --tail 50 <external-container-name> 2>&1 | tail -20
```

Look for:
- `zap-stream-external` container (the API service)
- `db` container (MariaDB)
- A Nostr relay container (e.g. `sw2-relay`, `strfry`) on the port matching `$NOSTR_RELAY_URL`

**Ensure the Nostr relay is running.** The relay (e.g. `sw2-relay`) is managed separately from the external stack's docker compose. If it shows as `Exited`, start it:

```bash
# Check relay status
docker ps -a | grep -E '(relay|sw2|strfry)'

# Start if stopped
docker start sw2-relay
```

The relay must be running before proceeding — the external service publishes Nostr events to it, and the e2e tests query it to verify events were published. Note: the relay uses NIP-42 authentication, which is handled by the `query_nostr_events_auth.js` helper script.

Take down and rebuild the stack so the service picks up the new tunnel URL and any code changes.

```bash
cd docs/deploy

# Stop existing containers
docker compose -f docker-compose.external.yaml -f docker-compose.override.yml down

# To also wipe the database for a fully clean slate:
# docker compose -f docker-compose.external.yaml -f docker-compose.override.yml down -v

# Build from source and start
docker compose -f docker-compose.external.yaml -f docker-compose.override.yml up -d --build

# Watch logs to confirm healthy startup
docker compose -f docker-compose.external.yaml -f docker-compose.override.yml logs -f
```

**What to look for in startup logs:**
- `Webhook created for <url>` or `Webhook notification url already registered` — confirms the service's own webhook endpoint is registered with Cloudflare
- `listening on 0.0.0.0:8080` — HTTP server ready
- No database connection errors

Press Ctrl+C to stop following logs once startup looks healthy.

**Verify everything is ready before proceeding:**

```bash
# Confirm containers are running
docker ps | grep -E '(zap-stream-external|db)'

# Confirm API is responding (should return JSON)
curl -s http://localhost:${ZS_API_PORT:-8080}/api/v1/time

# Confirm relay is reachable (uses NIP-42 auth; should return events or empty array)
node scripts/query_nostr_events_auth.js 30311 --since $(date +%s) --relay ${NOSTR_RELAY_URL:-ws://localhost:3334}
```

---

## Step 5: Update Cloudflare Webhook Notification URL

This is a separate configuration from the service's self-registered webhook. Cloudflare has a **Notifications** system that sends `live_input.connected`, `live_input.disconnected`, and `live_input.errored` events. This must point to your tunnel URL.

1. Go to the **Cloudflare Dashboard** > **Notifications** > **Destinations** > **Webhooks**
2. Find or create the webhook destination used for Stream Live Input notifications
3. Update the webhook URL to: `https://<your-tunnel-url>/api/v1/webhook/cloudflare`
4. Save the webhook destination
5. Go to **Notification Policies** and ensure the "Stream Live Input" notification policy is **enabled** and using this webhook destination

**Verify the tunnel is reachable** by hitting a known endpoint:

```bash
curl -s -o /dev/null -w "%{http_code}" https://<your-tunnel-url>/api/v1/time
```

Should return `200`. (Note: the webhook endpoint itself only accepts POST with a valid body, so don't use it as a health check.)

Without this step, stream start/end lifecycle events will not fire — the e2e tests will hang waiting for webhooks.

---

## Step 6: Run E2E Test Scripts

**Check environment overrides** before running tests. The test scripts default to `DB_ROOT_PASSWORD=root` and `ZS_API_PORT=8080`, but your docker override may differ:

```bash
# Check DB password and port mapping from the override
grep -E 'DB_ROOT_PASSWORD|ports' docs/deploy/docker-compose.override.yml
```

Export any values that differ from defaults:

```bash
export DB_ROOT_PASSWORD=<password-from-override>
export ZS_API_PORT=<host-port-from-override>   # e.g. 8090 if override maps "8090:8080"
```

Install Node.js dependencies if not already done:

```bash
cd scripts && npm install && cd ..
```

**(Optional) Set up `.env` for Cloudflare API direct validation:**

The `test-external-custom-keys-e2e.sh` script can validate custom keys directly against the Cloudflare API. This requires credentials in `scripts/.env`:

```bash
cp scripts/.env.example scripts/.env
# Edit scripts/.env with your CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN
```

Run the test scripts in this order. Each script is self-contained and exits with the number of failed tests as the exit code (0 = all passed). This makes them suitable for CI pipelines: `./scripts/test-external-e2e.sh || echo "Tests failed"`.

### 6a. Single-user lifecycle (16 tests, ~3-4 min)

```bash
./scripts/test-external-e2e.sh
```

**Tests:**
1. Prerequisites (node, jq, ffmpeg, Docker, containers)
2. Initial DB state (`external_id` column check)
3. API call with NIP-98 auth creates/reuses Live Input
4. Database contains valid `external_id` (32 hex chars)
5. RTMPS endpoint validation (`rtmps://` URL + stream key)
6. SRT endpoint validation (`srt://` URL + `streamid=...&passphrase=...`) — skips if SRT unavailable
7. Idempotency (second API call returns same credentials)
8. Custom key creation and listing via `/api/v1/keys`
9. Stream via RTMPS (30s FFmpeg test pattern)
10. Webhook START (container logs: `live_input.connected`)
11. LIVE Nostr 30311 event (status=live, streaming tag, starts tag, no ends tag)
12. End stream + webhook END (`live_input.disconnected`)
13. ENDED Nostr 30311 event (status=ended, ends tag, streaming tag removed)
14. Stream with custom key + webhook verification
15. Custom key Nostr metadata (title, summary, content tags from creation request)
16. Custom key ENDED Nostr event

### 6b. Multi-user concurrent streaming (14 tests, ~4-5 min)

```bash
./scripts/test-external-multi-user-e2e.sh
```

**Tests:**
1. Prerequisites
2. Database setup (two test users)
3. API credentials for both users
4. Unique `external_id` per user (different Cloudflare Live Inputs)
5. User A starts streaming
6. User A webhook START
7. User B starts streaming (concurrent with User A)
8. User B webhook START (>= 2 total connected events)
9. Both users have LIVE Nostr events on relay
10. Stream isolation: stop User A, User B continues
11. User A disconnect webhook + User B still alive
12. Stop User B (>= 2 total disconnected events)
13. Both users have ENDED Nostr events
14. UID persistence (external_ids unchanged after full lifecycle)

### 6c. Custom stream keys (11 tests, ~2-3 min)

```bash
./scripts/test-external-custom-keys-e2e.sh
```

**Tests:**
1. Prerequisites
2. Create custom key with metadata (title, summary, tags)
3. Create second custom key (verifies uniqueness)
4. List all keys (both present, unique stream_ids)
5. Cloudflare API direct validation (optional — requires `.env` with CF credentials)
6. Stream using custom key via RTMPS
7. Webhook START for custom key stream
8. LIVE Nostr 30311 event carries custom metadata (title, summary, tags, status, streaming URL)
9. End stream + webhook END
10. ENDED Nostr 30311 event (status=ended, ends tag, streaming removed)
11. Keys persist after stream lifecycle

**If any e2e tests fail**, check:
- Is the tunnel still alive? (repeat Step 2)
- Is the CF notification webhook pointing to the right URL? (repeat Step 5)
- Are containers running? `docker ps | grep -E '(zap-stream-external|db)'`
- Container logs: `docker logs <external-container> --tail 50`

---

## Step 7: Manual Smoke Test

After automated tests pass, do a manual test with a real streaming app to verify the user experience end-to-end.

### Setup

1. Open a stream viewer (e.g. zap.stream or your client app)
2. Open the service logs in a terminal:
   ```bash
   cd docs/deploy
   docker compose -f docker-compose.external.yaml -f docker-compose.override.yml logs -f
   ```

### Test

1. Start a live stream from a real device (iPhone, OBS, etc.) using the RTMPS URL from the API
2. Confirm in the logs:
   - `live_input.connected` webhook received
   - Single stream event published (not duplicates)
   - `Checking 1 live streams..` in the poller
3. Confirm in the viewer:
   - Stream appears as live
   - Playback works
4. Let the stream run for at least 2-3 minutes
5. Stop the stream
6. Confirm in the logs:
   - Stream ended (either via poller detecting `Disconnected` status or via `live_input.disconnected` webhook)
   - Recording webhook received (`Video Asset ready`)
   - Final Nostr event published with `status=ended`
7. Confirm in the viewer:
   - Stream shows as ended
   - Replay/recording is available

### What to watch for

- Only **one** stream record created per connection (not duplicates)
- Only **one** Nostr event published at stream start (not repeated every 30s)
- No `ERROR` lines in logs (warnings are OK)
- Recording/replay saves correctly after stream ends

---

## Environment Variables

All e2e scripts accept configuration via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `ZS_API_PORT` | `8080` | Port the external API listens on |
| `ZS_EXTERNAL_CONTAINER` | auto-detect | Docker container name for `zap-stream-external` |
| `ZS_DB_CONTAINER` | auto-detect | Docker container name for MariaDB |
| `DB_ROOT_PASSWORD` | `root` | MariaDB root password (must match the value in `docker-compose.override.yml`) |
| `NOSTR_RELAY_URL` | `ws://localhost:3334` | WebSocket URL of the Nostr relay |

Example with overrides:

```bash
ZS_API_PORT=8090 \
ZS_EXTERNAL_CONTAINER=my-stack-zap-stream-external-1 \
ZS_DB_CONTAINER=my-stack-db-1 \
NOSTR_RELAY_URL=ws://relay.example.com:7766 \
./scripts/test-external-e2e.sh
```

---

## Helper Scripts

These are used internally by the test scripts. You can also use them standalone for debugging.

### `sign_nip98.js` — NIP-98 Authentication

Creates a signed Nostr event (kind 27235) for HTTP authentication.

```bash
node scripts/sign_nip98.js <nsec> <url> <method>

# Example:
node scripts/sign_nip98.js nsec1... http://localhost:8080/api/v1/account GET
# Outputs JSON event, pipe through `base64` for Authorization header
```

### `decode_npub.js` — npub to Hex Conversion

```bash
node scripts/decode_npub.js <npub>

# Example:
node scripts/decode_npub.js npub1u0mm82x7muct7cy8y7urztyctgm0r6k27gdax04fa4q28x7q0shq6slmah
# Outputs: e3f7b3a8de...
```

### `query_nostr_events_auth.js` — Nostr Relay Query

Queries a Nostr relay for events with NIP-42 authentication support.

```bash
node scripts/query_nostr_events_auth.js [kind] [--since TIMESTAMP] [--relay URL]

# Examples:
node scripts/query_nostr_events_auth.js 30311
node scripts/query_nostr_events_auth.js 30311 --since 1700000000
node scripts/query_nostr_events_auth.js 30311 --since 1700000000 --relay ws://localhost:7766
```

### `test-srt-ingest.sh` — Interactive SRT Ingest Test

Interactive script for testing SRT ingest directly with the Cloudflare API. Requires Cloudflare credentials via environment or interactive prompt.

```bash
CF_TOKEN=xxx CF_ACCOUNT=yyy ./scripts/test-srt-ingest.sh
```

---

## Troubleshooting

**Docker daemon is not running:** If `docker ps` fails with "failed to connect to the docker API", start Docker Desktop with `open -a Docker` (macOS) and wait for it to be ready before proceeding.

**Cargo tests fail:** Fix before proceeding. E2e tests will not produce useful results against broken code.

**Tunnel dead:** `curl` returns `000` or times out. Kill old cloudflared, start new tunnel, update docker override (Step 3), restart stack (Step 4), update CF notification webhook (Step 5).

**Webhooks not received:** Most common cause. Verify: (a) tunnel is alive, (b) `APP__PUBLIC_URL` in docker override matches tunnel, (c) CF notification webhook URL in dashboard matches tunnel, (d) notification policy is enabled.

**Container not found:** Override with `ZS_EXTERNAL_CONTAINER=<name>` and `ZS_DB_CONTAINER=<name>`.

**Nostr events not found:** Verify the relay URL matches what's configured in `config.yaml` under `relays:`. Check logs for `Published stream event`.

**FFmpeg fails immediately:** Verify Cloudflare credentials are valid and the Live Input exists.

**Timing issues:** Cloudflare webhooks can take 5-30 seconds. If tests fail intermittently on webhook checks, the wait times may need increasing.

---

## Database Reference

Tests interact with MariaDB `zap_stream` database via `docker exec`. Key tables:

| Table | Key Columns | Purpose |
|-------|-------------|---------|
| `user` | `pubkey`, `external_id`, `balance` | User accounts; `external_id` = CF Live Input UID |
| `user_stream` | `id` (UUID), `user_id`, `state`, `starts`, `ends` | Stream records |
| `user_stream_key` | `user_id`, `key`, `external_id`, `stream_id` | Custom stream keys; `external_id` = CF Live Input UID (used for webhook matching) |

## Nostr Event Reference

Tests validate kind 30311 (NIP-53 Live Event) tags:

**When live:** `d`=stream UUID, `status`=`live`, `starts`=timestamp, `streaming`=playback URL, `p`=host pubkey, `service`=API URL

**When ended:** `status`=`ended`, `ends`=timestamp, `streaming` tag removed, `recording`=recording URL (if ready)

**Custom key metadata:** `title`, `summary` from key creation request, `t`=content tags
