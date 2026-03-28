# Testing Playbook for zap-stream-external

This document covers the full testing workflow for `zap-stream-external`, from unit tests through to manual smoke testing. Follow the steps in order.

## Architecture

The external backend (`zap-stream-external`) replaces local RTMP ingestion with Cloudflare Stream. It exposes an HTTP API on a configurable port (default 8080), processes Cloudflare webhooks for stream lifecycle events, and publishes Nostr kind 30311 events to relay(s).

The test harness validates the full lifecycle: API authentication, Cloudflare Live Input provisioning, RTMPS/SRT streaming, webhook processing, Nostr event publishing, and custom stream key management.

```
┌──────────────┐     NIP-98 auth      ┌─────────────────────────┐
│  Rust E2E     │ ──────────────────── │  zap-stream-external    │
│  Tests        │     HTTP requests    │  (Docker container)     │
│  (cargo test) │ ◄─────────────────── │                         │
│               │     JSON responses   │  Port: $ZS_API_PORT     │
└──────┬───────┘                       └────────┬────────────────┘
       │                                        │
       │  FFmpeg RTMPS ──────────────────── Cloudflare Stream
       │                                        │
       │                                   Webhooks (connected/
       │                                    disconnected)
       │                                        │
       │  nostr-sdk ─────────────────── Nostr Relay ($NOSTR_RELAY_URL)
       │                                        │
       │  sqlx (MariaDB) ──────────────── MariaDB (localhost:3306)
       └────────────────────────────────────────┘
```

## Prerequisites

- **Rust/Cargo** (for running both unit tests and e2e tests)
- **ffmpeg** (test stream generation)
- **Docker** (container access for logs and DB queries)
- **Running external stack**: `zap-stream-external` and `db` containers must be up
- **Cloudflare credentials**: Real Cloudflare account with Stream enabled (configured in the external service's `config.yaml`)
- **Nostr relay**: A relay accessible at `$NOSTR_RELAY_URL` that the external service publishes to
- **Webhook reachability**: The external service's `public_url` must be reachable from Cloudflare's servers (use cloudflared tunnel or similar for local dev)

## Quick Reference

| Step | Action | Who |
|------|--------|-----|
| 1 | Run cargo unit tests | AI Agent |
| 2 | Check cloudflared tunnel liveness | AI Agent |
| 3 | Update docker override with tunnel URL | AI Agent |
| 4 | Restart docker stack | AI Agent |
| 5 | Update Cloudflare webhook notification URL | Developer (in CF dashboard) |
| 6 | Run Rust e2e tests | AI Agent |
| 7 | Manual smoke test | Developer |

---

## Step 1: Run Cargo Unit Tests

Before anything else, confirm the code compiles and all unit tests pass.

```bash
cargo test -p zap-stream-external
```

**Expected result:** All unit tests pass (0 failed). The 3 e2e tests will show as "ignored" — this is correct. Pre-existing warnings are OK. If any tests fail, fix them before proceeding.

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

**Ensure the Nostr relay is running.** The relay is managed separately from the external stack's docker compose. If it shows as `Exited`, start it:

```bash
docker start sw2-relay
```

Take down and rebuild the stack so the service picks up the new tunnel URL and any code changes.

```bash
cd docs/deploy

# Stop existing containers
docker compose -f docker-compose.external.yaml -f docker-compose.override.yml down

# Build from source and start
docker compose -f docker-compose.external.yaml -f docker-compose.override.yml up -d --build

# Watch logs to confirm healthy startup
docker compose -f docker-compose.external.yaml -f docker-compose.override.yml logs -f
```

**What to look for in startup logs:**
- `Webhook created for <url>` or `Webhook notification url already registered`
- `listening on 0.0.0.0:8080` — HTTP server ready
- No database connection errors

---

## Step 5: Update Cloudflare Webhook Notification URL

This is a separate configuration from the service's self-registered webhook. Cloudflare has a **Notifications** system that sends `live_input.connected`, `live_input.disconnected`, and `live_input.errored` events. This must point to your tunnel URL.

1. Go to the **Cloudflare Dashboard** > **Notifications** > **Destinations** > **Webhooks**
2. Find or create the webhook destination used for Stream Live Input notifications
3. Update the webhook URL to: `https://<your-tunnel-url>/api/v1/webhook/cloudflare`
4. Save the webhook destination
5. Go to **Notification Policies** and ensure the "Stream Live Input" notification policy is **enabled** and using this webhook destination

Without this step, stream start/end lifecycle events will not fire — the e2e tests will hang waiting for webhooks.

---

## Step 6: Run E2E Tests

The e2e tests are Rust integration tests in `crates/zap-stream-external/tests/`. They are gated with `#[ignore]` and only run when explicitly requested.

**Check environment overrides** before running tests:

```bash
# Check DB password and port mapping from the override
grep -E 'DB_ROOT_PASSWORD|ports' docs/deploy/docker-compose.override.yml
```

Export any values that differ from defaults:

```bash
export DB_ROOT_PASSWORD=<password-from-override>
export ZS_API_PORT=<host-port-from-override>   # e.g. 8090 if override maps "8090:8080"
```

**(Optional) Set Cloudflare credentials for direct API validation:**

```bash
export CLOUDFLARE_API_TOKEN=<your-token>
export CLOUDFLARE_ACCOUNT_ID=<your-account-id>
```

### 6a. Single-user lifecycle (16 steps, ~3-4 min)

```bash
cargo test --test e2e_single_user -- --ignored --nocapture
```

**Steps:**
1. Prerequisites (ffmpeg, Docker, containers)
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

### 6b. Multi-user concurrent streaming (14 steps, ~4-5 min)

```bash
cargo test --test e2e_multi_user -- --ignored --nocapture
```

**Steps:**
1. Prerequisites
2. Database setup (two test users)
3. API credentials for both users
4. Unique `external_id` per user (different Cloudflare Live Inputs)
5. User A starts streaming
6. User A webhook START (isolation: User B not triggered)
7. User B starts streaming (concurrent with User A)
8. User B webhook START (both present)
9. Both users have LIVE Nostr events with distinct d-tags
10. Stream isolation: stop User A, User B continues
11. User A disconnect webhook + User B still alive + Nostr verification
12. Stop User B
13. Both users have ENDED Nostr events with ends tags
14. UID persistence (external_ids unchanged after full lifecycle)

### 6c. Custom stream keys (11 steps, ~2-3 min)

```bash
cargo test --test e2e_custom_keys -- --ignored --nocapture
```

**Steps:**
1. Prerequisites
2. Create custom key with metadata (title, summary, tags)
3. Create second custom key (verifies uniqueness)
4. List all keys (both present, unique stream_ids)
5. Cloudflare API direct validation (optional — requires CLOUDFLARE_API_TOKEN env var)
6. Stream using custom key via RTMPS
7. Webhook START for custom key stream
8. LIVE Nostr 30311 event carries custom metadata (title, summary, tags, status, streaming URL)
9. End stream + webhook END
10. ENDED Nostr 30311 event (status=ended, ends tag, streaming removed)
11. Keys persist after stream lifecycle

### Run all e2e tests at once

```bash
cargo test -p zap-stream-external -- --ignored --nocapture
```

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
   - Stream ended (either via poller or `live_input.disconnected` webhook)
   - Recording webhook received (`Video Asset ready`)
   - Final Nostr event published with `status=ended`
7. Confirm in the viewer:
   - Stream shows as ended
   - Replay/recording is available

---

## Environment Variables

All e2e tests accept configuration via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `ZS_API_PORT` | `8080` | Port the external API listens on |
| `ZS_EXTERNAL_CONTAINER` | auto-detect | Docker container name for `zap-stream-external` |
| `ZS_DB_CONTAINER` | auto-detect | Docker container name for MariaDB |
| `DB_ROOT_PASSWORD` | `root` | MariaDB root password |
| `NOSTR_RELAY_URL` | `ws://localhost:3334` | WebSocket URL of the Nostr relay |
| `CLOUDFLARE_API_TOKEN` | (none) | Optional: for direct CF API validation in custom keys test |
| `CLOUDFLARE_ACCOUNT_ID` | (none) | Optional: for direct CF API validation in custom keys test |

Example with overrides:

```bash
ZS_API_PORT=8090 \
ZS_EXTERNAL_CONTAINER=my-stack-zap-stream-external-1 \
ZS_DB_CONTAINER=my-stack-db-1 \
NOSTR_RELAY_URL=ws://relay.example.com:7766 \
cargo test -p zap-stream-external -- --ignored --nocapture
```

---

## Troubleshooting

These are steps for the AI agent to take autonomously — do not ask the user to perform them.

**Docker daemon is not running:** If `docker ps` fails, start Docker Desktop with `open -a Docker` (macOS), then poll `docker info` every 5 seconds until it succeeds before proceeding.

**Cargo tests fail:** Fix before proceeding. E2e tests will not produce useful results against broken code.

**Tunnel dead:** `curl` returns `000` or times out. Kill old cloudflared, start new tunnel, update docker override (Step 3), restart stack (Step 4), update CF notification webhook (Step 5).

**Webhooks not received:** Most common cause. Verify: (a) tunnel is alive, (b) `APP__PUBLIC_URL` in docker override matches tunnel, (c) CF notification webhook URL in dashboard matches tunnel, (d) notification policy is enabled.

**Container not found:** Override with `ZS_EXTERNAL_CONTAINER=<name>` and `ZS_DB_CONTAINER=<name>`.

**Nostr events not found:** Verify the relay URL matches what's configured in `config.yaml` under `relays:`. Check logs for `Published stream event`.

**FFmpeg fails immediately:** Verify Cloudflare credentials are valid and the Live Input exists.

**Timing issues:** Cloudflare webhooks can take 5-30 seconds. If tests fail intermittently on webhook checks, the wait times may need increasing.

---

## Database Reference

Tests interact with MariaDB `zap_stream` database via `sqlx`. Key tables:

| Table | Key Columns | Purpose |
|-------|-------------|---------|
| `user` | `pubkey`, `external_id`, `balance` | User accounts; `external_id` = CF Live Input UID |
| `user_stream` | `id` (UUID), `user_id`, `state`, `starts`, `ends` | Stream records |
| `user_stream_key` | `user_id`, `key`, `external_id`, `stream_id` | Custom stream keys; `external_id` = CF Live Input UID |

## Nostr Event Reference

Tests validate kind 30311 (NIP-53 Live Event) tags:

**When live:** `d`=stream UUID, `status`=`live`, `starts`=timestamp, `streaming`=playback URL, `p`=host pubkey, `service`=API URL

**When ended:** `status`=`ended`, `ends`=timestamp, `streaming` tag removed, `recording`=recording URL (if ready)

**Custom key metadata:** `title`, `summary` from key creation request, `t`=content tags
