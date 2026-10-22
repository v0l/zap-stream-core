# AVPlayer LL-HLS harness

Reproduces iOS/macOS playback problems using the real AVFoundation player on a
Mac, instead of guessing from hls.js behaviour.

- `player.swift`: a headless AVPlayer. It plays a URL for N seconds and prints
  item status, stalls, and every `errorLog` entry (CoreMedia error codes and
  URIs). It exits 0 if the playhead advanced for at least half the run.
- `rec.py` / `all.py`: mitmproxy addons that log every request with its
  Range header, status and latency, and every blocking playlist reload with
  what it returned.
- `fix.py` / `block.py`: addons that emulate the server-side fixes (RFC 8673
  range clamping and correct blocking-reload semantics) in front of the
  current production server. This lets you A/B test a fix on a real
  AVPlayer before deploying it.
- `abx.sh <label> <secs> [addon args]`: runs one A/B leg and prints a summary.

## Setup (macOS)

```sh
swiftc -O player.swift -o player
uv venv -p 3.12 venv12 && uv pip install -p venv12 mitmproxy
# mitmproxy must run on a Python with modern OpenSSL. With the system Python
# (LibreSSL 2.8) ALPN never negotiates h2, so AVPlayer disables LL-HLS
# ("Server must support http2") and the bugs do not reproduce.
./venv12/bin/mitmdump   # once, to generate ~/.mitmproxy
# trust the CA (needs GUI approval; remove it again afterwards):
security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db ~/.mitmproxy/mitmproxy-ca-cert.pem
```

Edit the stream URL in `abx.sh`, then:

```sh
./abx.sh prod 60                              # production as-is
./abx.sh fixed 60 -s fix.py -s block.py       # production + emulated fix
./player https://api-uk.zap.stream/<id>/hls/live.m3u8 60   # direct, no proxy
```

## What to look for in the player output

| CoreMedia error | Meaning |
|---|---|
| `-12939 HTTP 416` | A segment or part range request was rejected. AVPlayer fails the item (`-11850 server is not correctly configured`). |
| `-15410 Invalid server blocking reload behavior for low latency` | A `_HLS_msn`/`_HLS_part` reload was answered before the requested part existed. |
| `-15410 Server must support http2 ECN and SACK` | Connection is HTTP/1.1. LL-HLS is disabled, which is expected through a non-h2 proxy. |
| `-12318 Segment exceeds specified bandwidth` | Measured segment bitrate is above the variant's BANDWIDTH. |
