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
- Permissions: **Account** → **Stream** → **Edit**
- Account resources: **Include** → select your account
- Set `cloudflare.token` and `cloudflare.account_id` in your config

### 2. Webhook registration

The service automatically registers its webhook URL (`{public_url}/api/v1/webhook/cloudflare`) with Cloudflare on startup. If no webhook exists yet (fresh account), it will create one. If one exists with the correct URL, it skips registration.

**Note**: Cloudflare supports only **one webhook URL per account**. If multiple instances (e.g. staging and production) share a Cloudflare account, each startup will overwrite the other's webhook URL. Use separate Cloudflare accounts for separate environments.

### 3. Enable notification policies

In the Cloudflare dashboard, create a **Stream Live Input** notification policy:
- **Notifications** → **Add** → **Stream** → **Stream Live Input**
- Select your webhook destination
- Leave "Stream Live IDs" blank (applies to all inputs)
- Enable event types:
  - `live_input.connected`
  - `live_input.disconnected`

This is a one-time dashboard configuration — the API token does not create it.

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
