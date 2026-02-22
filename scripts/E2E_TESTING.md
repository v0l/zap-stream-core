# E2E Test Harness for zap-stream-external

End-to-end tests for the external Cloudflare Stream backend (`zap-stream-external`).

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
- **Running external stack**: `zap-stream-external` and `db` containers must be up
- **Cloudflare credentials**: Real Cloudflare account with Stream enabled (configured in the external service's `config.yaml`)
- **Nostr relay**: A relay accessible at `$NOSTR_RELAY_URL` that the external service publishes to
- **Webhook reachability**: The external service's `public_url` must be reachable from Cloudflare's servers (use cloudflared tunnel or similar for local dev)

## Setup

```bash
# 1. Install Node.js dependencies (one-time)
cd scripts && npm install && cd ..

# 2. (Optional) Copy .env for Cloudflare API direct validation
cp scripts/.env.example scripts/.env
# Edit scripts/.env with your CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN

# 3. Ensure the external stack is running
cd docs/deploy
docker-compose -f docker-compose.external.yaml up -d
cd ../..
```

## Environment Variables

All scripts accept the same configuration via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `ZS_API_PORT` | `8080` | Port the external API listens on |
| `ZS_EXTERNAL_CONTAINER` | auto-detect | Docker container name for `zap-stream-external` |
| `ZS_DB_CONTAINER` | auto-detect | Docker container name for MariaDB |
| `DB_ROOT_PASSWORD` | `root` | MariaDB root password |
| `NOSTR_RELAY_URL` | `ws://localhost:3334` | WebSocket URL of the Nostr relay |

Container auto-detection works by grepping `docker ps` output for `zap-stream-external` and `db-1`. Override with explicit names if your setup uses non-standard naming.

### Example with overrides

```bash
ZS_API_PORT=8090 \
ZS_EXTERNAL_CONTAINER=my-stack-zap-stream-external-1 \
ZS_DB_CONTAINER=my-stack-db-1 \
NOSTR_RELAY_URL=ws://relay.example.com:7766 \
./scripts/test-external-e2e.sh
```

## Test Scripts

### `test-external-e2e.sh` - Full Single-User Lifecycle (16 tests)

The primary test script. Covers the complete streaming lifecycle for a single user.

```bash
./scripts/test-external-e2e.sh
```

**Tests:**
1. Prerequisites (node, jq, ffmpeg, Docker, containers)
2. Initial DB state (`external_id` column check)
3. API call with NIP-98 auth creates/reuses Live Input
4. Database contains valid `external_id` (32 hex chars)
5. RTMPS endpoint validation (`rtmps://` URL + stream key)
6. SRT endpoint validation (`srt://` URL + `streamid=...&passphrase=...`) - skips if SRT unavailable
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

**Duration:** ~3-4 minutes (dominated by Cloudflare webhook latency).

### `test-external-multi-user-e2e.sh` - Multi-User Concurrent Streaming (14 tests)

Tests two users streaming simultaneously to verify isolation and correct webhook routing.

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

**Duration:** ~4-5 minutes.

### `test-external-custom-keys-e2e.sh` - Custom Stream Keys (11 tests)

Dedicated test for the custom keys feature (`POST /api/v1/keys`, `GET /api/v1/keys`).

```bash
./scripts/test-external-custom-keys-e2e.sh
```

**Tests:**
1. Prerequisites
2. Create custom key with metadata (title, summary, tags)
3. Create second custom key (verifies uniqueness)
4. List all keys (both present, unique stream_ids)
5. Cloudflare API direct validation (optional - requires `.env` with CF credentials)
6. Stream using custom key via RTMPS
7. Webhook START for custom key stream
8. LIVE Nostr 30311 event carries custom metadata (title, summary, tags, status, streaming URL)
9. End stream + webhook END
10. ENDED Nostr 30311 event (status=ended, ends tag, streaming removed)
11. Keys persist after stream lifecycle

**Duration:** ~2-3 minutes.

## Helper Scripts

These are used internally by the test scripts. You can also use them standalone for debugging.

### `sign_nip98.js` - NIP-98 Authentication

Creates a signed Nostr event (kind 27235) for HTTP authentication.

```bash
node scripts/sign_nip98.js <nsec> <url> <method>

# Example:
node scripts/sign_nip98.js nsec1... http://localhost:8080/api/v1/account GET
# Outputs JSON event, pipe through `base64` for Authorization header
```

### `decode_npub.js` - npub to Hex Conversion

```bash
node scripts/decode_npub.js <npub>

# Example:
node scripts/decode_npub.js npub1u0mm82x7muct7cy8y7urztyctgm0r6k27gdax04fa4q28x7q0shq6slmah
# Outputs: e3f7b3a8de...
```

### `query_nostr_events_auth.js` - Nostr Relay Query

Queries a Nostr relay for events with NIP-42 authentication support.

```bash
node scripts/query_nostr_events_auth.js [kind] [--since TIMESTAMP] [--relay URL]

# Examples:
node scripts/query_nostr_events_auth.js 30311
node scripts/query_nostr_events_auth.js 30311 --since 1700000000
node scripts/query_nostr_events_auth.js 30311 --since 1700000000 --relay ws://localhost:7766
```

### `test-srt-ingest.sh` - Interactive SRT Ingest Test

Interactive script for testing SRT ingest directly with the Cloudflare API. Requires Cloudflare credentials via environment or interactive prompt.

```bash
CF_TOKEN=xxx CF_ACCOUNT=yyy ./scripts/test-srt-ingest.sh
```

## Exit Codes

All test scripts exit with the number of failed tests as the exit code:
- `0` = all tests passed
- `N` = N tests failed

This makes them suitable for CI pipelines: `./scripts/test-external-e2e.sh || echo "Tests failed"`.

## Database Details

Tests interact with the MariaDB `zap_stream` database via `docker exec`. Key tables:

- **`user`**: `pubkey` (binary), `external_id` (Cloudflare Live Input UID), `balance`
- **`user_stream`**: `id` (UUID), `user_id`, `state` (live/ended/planned), `starts`, `ends`, `title`, `summary`, `tags`
- **`user_stream_key`**: `id`, `user_id`, `key` (stream key), `external_id` (CF Live Input UID), `stream_id` (UUID)

The `external_id` column on `user` stores the Cloudflare Live Input UID for the user's default stream. The `external_id` column on `user_stream_key` stores the CF Live Input UID for custom keys (used for webhook matching).

## Nostr Event Verification

Tests validate kind 30311 (NIP-53 Live Event) tags published to the relay:

**When live:**
- `d` = stream UUID
- `status` = `live`
- `starts` = unix timestamp
- `streaming` = playback URL
- `p` = host pubkey
- `service` = API base URL

**When ended:**
- `status` = `ended`
- `ends` = unix timestamp
- `streaming` tag removed
- `recording` = recording URL (if Cloudflare recording ready)

**Custom key metadata (when set at key creation):**
- `title`, `summary` = from `event` field in POST /api/v1/keys request
- `t` = content tags from `event.tags` array

## Troubleshooting

**Container not found**: Override with `ZS_EXTERNAL_CONTAINER=<name>` and `ZS_DB_CONTAINER=<name>`.

**Webhook not received**: The external service's `public_url` in `config.yaml` must be reachable from Cloudflare. Check `docker logs <external-container>` for `Webhook created for` or `Webhook notification url already registered` on startup.

**Nostr events not found**: Verify the relay URL matches what's configured in the external service's `config.yaml` under `relays:`. Check logs for `Published stream event`.

**FFmpeg fails immediately**: Verify Cloudflare credentials are valid and the Live Input exists. The RTMPS URL and key come from Cloudflare's API via the external service.

**Timing issues**: Cloudflare webhooks can take 5-30 seconds. If tests fail intermittently on webhook checks, the wait times (20s for start, 15s for end) may need increasing for your network.
