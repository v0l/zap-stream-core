# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] - 2025-12-05

### Added

- Media over QUIC (MoQ) egress support with H264, H265, VP8, VP9 video and AAC/Opus audio codecs (#37)
- Multi-track streaming configuration API (`POST /api/v1/multi-track-config`) for OBS auto-configuration
- Hardware encoder support in multi-track config: NVENC, VAAPI, QSV with automatic fallback
- Frame reorder buffer for proper B-frame handling before encoding
- Admin endpoint to retrieve pipeline logs (`GET /api/v1/admin/pipeline-log/{stream_id}`) (#46)
- Redis-based viewer tracking with sorted sets for scalability
- Per-stream pipeline.log for detailed debug output
- Configurable minimum stream event update rate
- `get_user_live_streams` database method for retrieving active streams

### Changed

- Refactored `EndpointConfigEngine` for variant generation with deduplication across egress types
- Account balance now returned as `i64` to support negative balances (#40)
- Thumbnail generation uses time-based interval (5 minutes) instead of frame count
- Encoder settings now configured by egress requirements via `EncoderParam` enum
- `VideoVariant` and `AudioVariant` use `apply_params()` for configuration
- `StreamManager` supports Redis for distributed viewer tracking
- Switched to NWC crate for Nostr Wallet Connect
- Upgraded ffmpeg-rs-raw dependency
- Docker image now uses Debian trixie slim runner
- User blocking immediately stops all active streams for blocked user (#47)
- Init segment writing now occurs at startup
- Withdrawal feature behind `withdrawal` feature flag

### Fixed

- Frame PTS mangling instead of packet PTS for proper timing (#39)
- Monotonic PTS values with offset tracking per stream
- Encoder timebase set to 90k tbn for consistent timing
- HLS variant encoding issues
- Multi-track encoder settings configuration
- NV12 pixel format usage with GPU encoding
- Color space/range included in video variant parameters
- Default FPS to 30 when not detected from source
- Keyframe flag checking for thumbnails in copy-only pipelines (#44)
- Thumbnail generation for copy-only streams
- Audio always transcoded for copy streams for compatibility
- Negative cost/duration value prevention with validation
- Memory leak in pipeline processing
- AVIO crash with additional logging (#50)
- Crypto provider setup for TLS connections (#49)
- History endpoint 500 error from payment_type type mismatch (#45)
- Negative balance return value (#40)
- Stream image set on new stream; thumb used as image when no image set
- Empty strings removed from stream metadata
- Init segment flags for proper playback

### Removed

- Idle mode from pipeline runner (streams now end cleanly on EOF)
- Circuit breaker logic for decode failures
- `Idle` state from `RunnerState` enum
- Apt cache from Docker image