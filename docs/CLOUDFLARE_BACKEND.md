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

You will need a Cloudflare account with Live Streaming enabled.

1) **Create an API token** with access to Stream
2) **Set webhook destination** to your external service
3) **Enable notifications** for:
   - `live_input.connected`
   - `live_input.disconnected`
   - `live_input.errored`
   - `live_input.recording.ready`

Your external service will automatically register the webhook URL on startup, and Cloudflare should confirm it.

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
