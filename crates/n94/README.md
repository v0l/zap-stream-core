# N94 Broadcaster

A standalone NIP-5E broadcaster for streaming to Nostr using the N94 protocol.

## Overview

N94 is a streaming broadcaster that:
- Ingests video streams via RTMP, SRT, or test patterns
- Transcodes to multiple quality variants (1080p, 720p, 480p, 240p)
- Publishes stream segments to Blossom servers for distributed storage
- Broadcasts stream events to Nostr relays following NIP-5E protocol

## Quick Start

```bash
# install with cargo
cargo install n94 --git https://github.com/v0l/zap-stream-core

# or build the project from source
cargo build --release

# Run with minimal configuration
n94 \
  --nsec <your-nostr-private-key> \
  --blossom <blossom-server-url> \
  --title "My Stream"
```

## Required Arguments

- `--nsec`: Your Nostr private key in nsec format for publishing events
- `--blossom`: Blossom server URL(s) for uploading stream segments
- `--title`: Stream title

## Optional Arguments

### Stream Configuration
- `--summary`: Long description of the stream
- `--image`: Stream thumbnail image URL
- `--goal`: Stream goal or purpose
- `--hashtag`: Hashtags to add to the stream (can be repeated)

### Network Configuration
- `--relay`: Nostr relay URLs (defaults to damus.io, primal.net, nos.lol)
- `--listen`: Listen endpoints for ingress (default: `rtmp://localhost:1935`)
- `--nip53-bridge`: Bridge proxy for backwards compatible NIP-53 events

### Technical Configuration
- `--data-dir`: Directory for temporary files (default: `./out`)
- `--capability`: Video quality variants (default: 1080p/6M, 720p/4M, 480p/2M, 240p/1M)

## Example Usage

### Basic RTMP Stream
```bash
n94 \
  --nsec nsec1... \
  --blossom https://blossom.example.com \
  --title "Live Coding Session" \
  --summary "Building a Rust application" \
  --hashtag rust \
  --hashtag coding
```

### Multiple Quality Variants
```bash
n94 \
  --nsec nsec1... \
  --blossom https://blossom1.com \
  --blossom https://blossom2.com \
  --title "Conference Talk" \
  --capability variant:1080:8000000 \
  --capability variant:720:5000000 \
  --capability variant:480:3000000
```

### SRT Ingress
```bash
n94 \
  --nsec nsec1... \
  --blossom https://blossom.example.com \
  --listen srt://localhost:8554 \
  --title "SRT Stream Test"
```

### Test Pattern (for testing)
```bash
n94 \
  --nsec nsec1... \
  --blossom https://blossom.example.com \
  --listen test-pattern:// \
  --title "Test Stream"
```

## Streaming to N94

Once N94 is running, you can stream to it using:

**RTMP (default):**
```bash
ffmpeg -re -i input.mp4 -c copy -f flv rtmp://localhost:1935/live
```

**SRT:**
```bash
ffmpeg -re -i input.mp4 -c copy -f mpegts srt://localhost:8554
```

## Output

N94 will:
1. Accept your video stream
2. Transcode it to multiple quality variants
3. Upload HLS segments to configured Blossom servers
4. Publish stream events to Nostr relays
5. Generate stream manifests accessible via the configured data directory

## Dependencies

- Rust 1.70+
- FFmpeg (for transcoding)
- Network access to Blossom servers and Nostr relays