# Cloudflare Live Stream External Backend (zap-stream-external)

This repo includes an external backend binary (`zap-stream-external`) that replaces the core ingest backend when you want Cloudflare Live Stream ingestion. It runs its own HTTP API, publishes Nostr events, and processes webhooks from Cloudflare.

If you use the external backend:
- You run **zap-stream-external** instead of the core HTTP API.
- It connects to the same database and Lightning backend.
- Cloudflare provides ingest + recording; the external service manages stream state and publishes events.

## Summary of components

- **Database (MySQL/MariaDB)**: shared source of truth for users, streams, payments.
- **Lightning backend**: required for payments.
- **Nostr relays**: publish live event metadata.
- **zap-stream-external**: Cloudflare-backed API + webhook server.

## Configure zap-stream-external

The external binary loads `config.yaml` and can be overridden with environment variables using the `APP__` prefix (double underscore for nesting). Example:

- `APP__PUBLIC_URL=https://api.example.com`
- `APP__CLOUDFLARE__TOKEN=...`

Start from `crates/zap-stream-external/config.yaml` and update the following:

Required:
- `database`
- `public_url`
- `listen_http`
- `nsec`
- `relays[]`
- `cloudflare.token`
- `cloudflare.account_id`
- `payments` (LND, LNURL, etc.)

Optional (but recommended):
- `tos_url` (defaults to https://zap.stream/tos)
- `client_url` (defaults to https://zap.stream)
- `endpoints_public_hostname` (Cloudflare custom ingest domain)

## Cloudflare setup

You will need a Cloudflare account with **Stream** (paid subscription) enabled.

### 1. Create an API token

In the Cloudflare dashboard:
- **My Profile** → **API Tokens** → **Create Token** → **Custom token**
- Permissions:
  - **Account** → **Stream** → **Edit**
  - **Account** → **Notifications** → **Edit**
- Account resources: **Include** → select your account
- Set `cloudflare.token` and `cloudflare.account_id` in your config

Both permissions are required. Stream is for live input management and video asset webhooks. Notifications is for the live input event alerting policy (connected/disconnected).

### 2. Understand the two webhook systems

Cloudflare uses two separate webhook delivery mechanisms for Stream:

| System | What it delivers | How it's configured |
|--------|-----------------|-------------------|
| **Stream webhook** (`/stream/webhook` API) | Video Asset events (recording ready, thumbnail) | Auto-registered by the service on startup |
| **Notification policy** (Alerting API) | Live input events (`live_input.connected`, `live_input.disconnected`, `live_input.errored`) | One-time API setup per account (step 4 below) |

Both deliver to the same URL (`{public_url}/api/v1/webhook/cloudflare`) but via different Cloudflare systems.

### 3. Stream webhook registration (automatic)

The service automatically registers its webhook URL with Cloudflare on startup via `PUT /stream/webhook`. If no webhook exists yet (fresh account), it creates one. If one exists with the correct URL, it skips registration.

**Note**: Cloudflare supports only **one Stream webhook URL per account**. If multiple instances (e.g. staging and production) share a Cloudflare account, each startup will overwrite the other's webhook URL. Use separate Cloudflare accounts for separate environments.

### 4. Create notification policy for live input events (one-time setup)

Without this step, the service will NOT receive `live_input.connected` or `live_input.disconnected` events. Streams will only be tracked by the 30-second poller, and recordings will still work, but the live lifecycle will not be webhook-driven.

This is a one-time setup per Cloudflare account, done via the Alerting API. It does NOT require any zones or domains on the account.

**Step 1: Create a webhook destination**

```bash
curl -s -X POST \
  -H "Authorization: Bearer <API_TOKEN>" \
  -H "Content-Type: application/json" \
  "https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/alerting/v3/destinations/webhooks" \
  -d '{"name": "ZS Core Webhook", "url": "<PUBLIC_URL>/api/v1/webhook/cloudflare"}'
```

Response includes the webhook destination `id` — save it for step 2.

**Step 2: Create the notification policy**

```bash
curl -s -X POST \
  -H "Authorization: Bearer <API_TOKEN>" \
  -H "Content-Type: application/json" \
  "https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/alerting/v3/policies" \
  -d '{
    "name": "Stream Live Notifications",
    "description": "Notifies on live stream connect/disconnect/error",
    "enabled": true,
    "alert_type": "stream_live_notifications",
    "mechanisms": {
      "webhooks": [{"id": "<WEBHOOK_DESTINATION_ID>"}]
    },
    "filters": {}
  }'
```

Empty `filters` means all live inputs are monitored. To restrict to specific inputs, add `"input_id": ["<input_id>"]` inside `filters`.

**Verify the setup:**

```bash
# List notification policies
curl -s -H "Authorization: Bearer <API_TOKEN>" \
  "https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/alerting/v3/policies" | jq .

# List webhook destinations
curl -s -H "Authorization: Bearer <API_TOKEN>" \
  "https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/alerting/v3/destinations/webhooks" | jq .
```

**Note**: The Cloudflare dashboard Notifications UI may not show these options on accounts without zones. The API works regardless.

## Custom ingest domain (optional)

Cloudflare allows custom RTMPS domains. If you use this:

- Configure the CNAME in your DNS provider.
- Add the domain in Cloudflare (Stream > Live inputs > Custom Input Domains).
- Set `endpoints_public_hostname` in `zap-stream-external` config.

This will update the RTMPS ingest URL returned by `/api/v1/account`.

## SRT ingest (if enabled in Cloudflare)

If your Cloudflare Live Input includes SRT details, the external backend will return
both RTMPS and SRT endpoints in `/api/v1/account`. Clients can choose transport
based on the URL scheme.

## Ingest endpoint configuration (recommended)

Cloudflare streams have a fixed capability set. We recommend a single ingest endpoint:
- `variant:720:30`
- `dvr:720`

Use the admin UI to remove extra endpoints and keep one matching Cloudflare capabilities.

## Example external compose

See `docs/deploy/docker-compose.external.yaml` for a working example.

To use local overrides (recommended), create your own `docker-compose.override.yml`
in your project folder and run:

```bash
docker compose -f docs/deploy/docker-compose.external.yaml -f docker-compose.override.yml up -d
```
