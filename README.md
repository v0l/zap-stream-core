# zap.stream core

Rust zap.stream core streaming server

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/v0l/zap-stream-core)

![diagram](./crates/zap-stream/zap.stream.svg)

## Documentation

- [API Documentation](./docs/API.md) - Complete REST API reference
- [Deployment Guide](./docs/DEPLOYMENT.md) - Production deployment with Docker

## Deploying

The easiest way to deploy `zap-stream-core` is to use [Docker Compose](https://docs.docker.com/compose/),
copy the files from `docs/deploy/` to your machine.

Then edit the config file `compose-config.yaml` to suit your needs.

Make sure to modify the following values:

- `admin_pubkey`: Your regular nostr pubkey in hex, this must be set to be able to login with the admin UI.
- `endpoints_public_hostname`: The hostname of the ingress endpoints (RTMP), this is returned by the API so if its
  incorrect your users may not be able to connect. This can be the same as `public_url` if they both resolve to the
  public IP of the server.
- `public_url`: The hostname used for the API (HTTP), this can be different than the above hostname if you wish to proxy
  the API via cloudflare for example while using the IP directly for RTMP, just know that the API will return the
  `endpoint_public_hostname` when queried so make sure not to leak the public IP.
- `overseer.nsec`: This is the NSEC which is used to publish stream events on nostr.
- `overseer.database`: Add your database root password here if you modified it in the compose config. With the format
  `user:password@db`
- `overseer.payments`: Add your payments backend, this is how you can topup balances in the system for streaming.
- `overseer.advertise`: If you want to make your server discoverable automatically on zap.stream, uncomment this config.

Once configured run the below command to start the system:

```bash
docker compose up -d
```

This config will bind the public ports `TCP/80` and `TCP/1935`, you can remove these port
mappings if you wish to use the tunnel setup below.

- `TCP/80`: Main HTTP API and content server for stream output
- `TCP/1935`: RTMP ingress port

In most cases you need to have valid SSL certificates in order for your instance to be accessible to other users,
SSL setup is not covered in this document, the easiest solution is to proxy the API via cloudflare and use a different
hostname for the RTMP ingress endpoints.

Open the admin UI here to manage your server: https://admin.zap.stream

### No Public IP?

If you want to run `zap-stream-core` without a public IP behind Cloudflare you can run the following command
**AFTER** you modify the `docker-compose.yaml` to include your cloudflared tunnel token.

```bash
docker compose --profile cloudflared up -d
```

This will proxy only the HTTP API, meaning that you still need a way to access the RTMP endpoints directly to live
stream with it. This kind of setup is good for home node runners, it allows you to host a live stream from your home
network.

**Again make sure not to leak any sensitive information via the `public_url` / `endpoints_public_url` in this
setup.**