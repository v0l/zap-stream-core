use crate::stream_manager::StreamManager;
use crate::viewer::ViewerTracker;
use anyhow::{Result, ensure};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum_extra::response::FileStream;
use http_range_header::{
    EndPosition, StartPosition, SyntacticallyCorrectRange, parse_range_header,
};
use serde::Deserialize;
use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;
use tracing::warn;
use uuid::Uuid;
use zap_stream_core::egress::hls::HLS_EGRESS_PATH;

#[derive(Clone)]
pub struct HlsRouter {
    base_path: PathBuf,
    stream_manager: StreamManager,
}

impl HlsRouter {
    const PLAYLIST_CONTENT_TYPE: &'static str = "application/vnd.apple.mpegurl";

    /// Validate a single user-supplied path component. Axum percent-decodes path
    /// segments, so values like ".." or "a/b" can appear here and would otherwise
    /// escape the media base directory (path traversal).
    fn safe_path_component(part: &str) -> Result<&str, (StatusCode, &'static str)> {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains('/')
            || part.contains('\\')
            || part.contains('\0')
        {
            return Err((StatusCode::BAD_REQUEST, "Invalid path"));
        }
        Ok(part)
    }

    pub fn new<P>(base_path: P, stream_manager: StreamManager) -> Router
    where
        P: Into<PathBuf>,
    {
        Router::new()
            .route(
                &format!("/{{stream}}/{}/live.m3u8", HLS_EGRESS_PATH),
                get(Self::get_master_playlist),
            )
            .route(
                &format!("/{{stream}}/{}/{{variant}}/live.m3u8", HLS_EGRESS_PATH),
                get(Self::get_variant_playlist),
            )
            .route(
                &format!("/{{stream}}/{}/{{variant}}/{{seg}}", HLS_EGRESS_PATH),
                get(Self::get_segment),
            )
            .with_state(Self {
                base_path: base_path.into(),
                stream_manager,
            })
    }

    async fn get_master_playlist(
        Path(stream): Path<Uuid>,
        State(this): State<HlsRouter>,
        headers: HeaderMap,
    ) -> Result<Response, (StatusCode, String)> {
        let client_ip = Self::get_client_ip(&headers);
        let user_agent = headers.get("user-agent").and_then(|h| h.to_str().ok());

        let token = ViewerTracker::generate_viewer_token(&client_ip, user_agent);

        // Read the playlist file
        let playlist_path = this
            .base_path
            .join(stream.to_string())
            .join(HLS_EGRESS_PATH)
            .join("live.m3u8");
        // NOTE: errors must carry a non-2xx status; a bare String rejection would
        // render as 200 OK with an error message body, which breaks HLS players.
        let playlist_content = tokio::fs::read(playlist_path).await.map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                format!("Failed to read playlist file: {}", e),
            )
        })?;

        // Parse and modify playlist to add viewer token to URLs
        let modified_content =
            Self::add_viewer_token_to_playlist(&playlist_content, &token).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to add playlist token to playlist: {}", e),
                )
            })?;

        let headers = [(CONTENT_TYPE, Self::PLAYLIST_CONTENT_TYPE)];
        Ok((
            headers,
            match modified_content {
                Cow::Borrowed(content) => content.to_string(),
                Cow::Owned(modified_content) => modified_content,
            },
        )
            .into_response())
    }

    async fn get_variant_playlist(
        Path((stream_id, variant)): Path<(String, String)>,
        Query(q): Query<ViewerTokenQuery>,
        State(this): State<HlsRouter>,
    ) -> Result<Response, (StatusCode, &'static str)> {
        let playlist_path = this
            .base_path
            .join(Self::safe_path_component(&stream_id)?)
            .join(HLS_EGRESS_PATH)
            .join(Self::safe_path_component(&variant)?)
            .join("live.m3u8");

        if let Some(vt) = q.vt.as_deref() {
            this.stream_manager.track_viewer(&stream_id, vt).await;
        }

        // LL-HLS blocking playlist reload (RFC 8216, Part 6.2.5.2):
        // when the client asks for a future media-sequence-number / part via
        // `_HLS_msn` and `_HLS_part`, hold the request until the playlist on disk
        // actually contains that part (or we time out). This is what lets players
        // run at sub-segment latency instead of polling.
        let content = if q.hls_msn.is_some() {
            Self::read_playlist_blocking(&playlist_path, q.hls_msn, q.hls_part)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?
        } else {
            tokio::fs::read(&playlist_path)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?
        };

        let headers = [
            (CONTENT_TYPE, Self::PLAYLIST_CONTENT_TYPE),
            // playlists are live and must never be cached by intermediaries
            (axum::http::header::CACHE_CONTROL, "no-cache, no-store"),
        ];
        Ok((headers, content).into_response())
    }

    /// Block until the variant playlist contains the requested (msn, part), then
    /// return its bytes. Falls back to whatever is current after a short timeout
    /// so a client is never left hanging if the stream stalls or ends.
    async fn read_playlist_blocking(
        path: &std::path::Path,
        want_msn: Option<u64>,
        want_part: Option<u64>,
    ) -> Result<Vec<u8>> {
        use std::time::Duration;
        use tokio::time::Instant;

        // Bound the wait. Players re-issue the blocking request, so a modest cap
        // keeps connections from piling up while still covering a full segment.
        let deadline = Instant::now() + Duration::from_secs(6);
        let want_msn = want_msn.unwrap_or(0);
        let want_part = want_part.unwrap_or(0);

        loop {
            let content = tokio::fs::read(path).await?;
            if Self::playlist_has_part(&content, want_msn, want_part) || Instant::now() >= deadline {
                return Ok(content);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    /// Returns true when the playlist already contains the requested part, i.e.
    /// the requested (msn, part) is <= the most recent part advertised.
    fn playlist_has_part(content: &[u8], want_msn: u64, want_part: u64) -> bool {
        let (_, pl) = match m3u8_rs::parse_playlist(content) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let pl = match pl {
            m3u8_rs::Playlist::MediaPlaylist(pl) => pl,
            // master playlist can't carry parts; don't block
            m3u8_rs::Playlist::MasterPlaylist(_) => return true,
        };

        // MSN of the first listed full segment
        let media_sequence = pl.media_sequence;
        // Count full segments and the partial segments that trail the last full one.
        let mut full_count: u64 = 0;
        let mut trailing_parts: u64 = 0;
        for seg in &pl.segments {
            match seg {
                m3u8_rs::MediaSegmentType::Full(_) => {
                    full_count += 1;
                    trailing_parts = 0;
                }
                m3u8_rs::MediaSegmentType::Partial(_) => {
                    trailing_parts += 1;
                }
                m3u8_rs::MediaSegmentType::PreloadHint(_) => {}
            }
        }

        // The in-progress segment (the one currently accumulating parts) has this MSN
        let in_progress_msn = media_sequence + full_count;
        if want_msn < in_progress_msn {
            // A later segment has already begun, so every part of `want_msn` exists.
            return true;
        }
        if want_msn == in_progress_msn {
            // available part indices are 0..trailing_parts-1
            return trailing_parts > want_part;
        }
        // requested a segment we haven't started yet
        false
    }

    async fn get_segment(
        State(this): State<HlsRouter>,
        Path((stream_id, variant, segment)): Path<(Uuid, String, String)>,
        headers: HeaderMap,
    ) -> Result<Response, (StatusCode, &'static str)> {
        let segment_path = this
            .base_path
            .join(stream_id.to_string())
            .join(HLS_EGRESS_PATH)
            .join(Self::safe_path_component(&variant)?)
            .join(Self::safe_path_component(&segment)?);

        let stream = FileStream::from_path(&segment_path)
            .await
            .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
        if let Some(r) = headers.get("range") {
            let r = r
                .to_str()
                .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid range"))?;
            if let Ok(ranges) = parse_range_header(r) {
                if ranges.ranges.len() > 1 {
                    warn!("Multipart ranges are not supported, fallback to non-range request");
                    Ok(stream.into_response())
                } else {
                    let file_size = stream.content_size.unwrap();
                    let single_range = ranges
                        .ranges
                        .into_iter()
                        .next()
                        .and_then(|r| Self::get_range(file_size, &r).ok())
                        .ok_or_else(|| {
                            (StatusCode::RANGE_NOT_SATISFIABLE, "Invalid range request")
                        })?;
                    Ok(stream.into_range_response(single_range.start, single_range.end, file_size))
                }
            } else {
                Err((StatusCode::BAD_REQUEST, "Invalid range"))
            }
        } else {
            Ok(stream.into_response())
        }
    }

    fn get_range(file_size: u64, header: &SyntacticallyCorrectRange) -> Result<Range<u64>> {
        const MAX_UNBOUNDED_RANGE: u64 = 1024 * 1024;
        let range_start = match header.start {
            StartPosition::Index(i) => {
                ensure!(i < file_size, "Range start out of range");
                i
            }
            StartPosition::FromLast(i) => file_size.saturating_sub(i),
        };
        let range_end = match header.end {
            EndPosition::Index(i) => {
                // HTTP ranges are inclusive: the max valid end index is file_size - 1
                ensure!(i < file_size, "Range end out of range");
                i
            }
            EndPosition::LastByte => {
                (file_size.saturating_sub(1)).min(range_start + MAX_UNBOUNDED_RANGE)
            }
        };
        Ok(range_start..range_end)
    }

    fn get_client_ip(headers: &HeaderMap) -> String {
        // Check common headers for real client IP
        if let Some(forwarded) = headers.get("x-forwarded-for")
            && let Ok(forwarded_str) = forwarded.to_str()
            && let Some(first_ip) = forwarded_str.split(',').next()
        {
            return first_ip.trim().to_string();
        }

        if let Some(real_ip) = headers.get("x-real-ip")
            && let Ok(ip_str) = real_ip.to_str()
        {
            return ip_str.to_string();
        }

        // use random string as IP to avoid broken view tracker due to proxying
        Uuid::new_v4().to_string()
    }

    fn add_viewer_token_to_playlist<'a>(
        content: &'a [u8],
        viewer_token: &str,
    ) -> Result<Cow<'a, str>> {
        // Parse the M3U8 playlist using the m3u8-rs crate
        let (_, playlist) = m3u8_rs::parse_playlist(content)
            .map_err(|e| anyhow::anyhow!("Failed to parse M3U8 playlist: {}", e))?;

        match playlist {
            m3u8_rs::Playlist::MasterPlaylist(mut master) => {
                // For master playlists, add viewer token to variant streams
                for variant in &mut master.variants {
                    variant.uri = Self::add_token_to_url(&variant.uri, viewer_token);
                }

                // Write the modified playlist back to string
                let mut output = Vec::new();
                master
                    .write_to(&mut output)
                    .map_err(|e| anyhow::anyhow!("Failed to write master playlist: {}", e))?;
                String::from_utf8(output)
                    .map(Cow::Owned)
                    .map_err(|e| anyhow::anyhow!("Failed to convert playlist to string: {}", e))
            }
            m3u8_rs::Playlist::MediaPlaylist(_) => Ok(Cow::Borrowed(str::from_utf8(content)?)),
        }
    }

    fn add_token_to_url(url: &str, viewer_token: &str) -> String {
        if url.contains('?') {
            format!("{}&vt={}", url, viewer_token)
        } else {
            format!("{}?vt={}", url, viewer_token)
        }
    }
}

#[derive(Deserialize)]
struct ViewerTokenQuery {
    #[serde(default)]
    pub vt: Option<String>,
    /// LL-HLS blocking reload: media sequence number being requested
    #[serde(rename = "_HLS_msn", default)]
    pub hls_msn: Option<u64>,
    /// LL-HLS blocking reload: partial segment index within `_HLS_msn`
    #[serde(rename = "_HLS_part", default)]
    pub hls_part: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative LL-HLS media playlist: media-sequence 10, two full
    /// segments (MSN 10, 11) followed by the in-progress segment (MSN 12) that
    /// currently has 2 published parts plus a preload hint for the third.
    const LL_PLAYLIST: &str = "#EXTM3U\n\
#EXT-X-VERSION:6\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:10\n\
#EXT-X-MAP:URI=\"init.mp4\"\n\
#EXT-X-PART-INF:PART-TARGET=0.5\n\
#EXT-X-SERVER-CONTROL:PART-HOLD-BACK=1.500,CAN-BLOCK-RELOAD=YES\n\
#EXTINF:2,\n10.m4s\n\
#EXTINF:2,\n11.m4s\n\
#EXT-X-PART:DURATION=0.5,URI=\"12.m4s\",BYTERANGE=\"100@0\",INDEPENDENT=YES\n\
#EXT-X-PART:DURATION=0.5,URI=\"12.m4s\",BYTERANGE=\"100@100\"\n\
#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"12.m4s\",BYTERANGE-START=200\n";

    #[test]
    fn ll_playlist_parses() {
        // The tags we emit must round-trip through the parser the server itself uses.
        let (_, pl) = m3u8_rs::parse_playlist(LL_PLAYLIST.as_bytes()).expect("valid playlist");
        assert!(matches!(pl, m3u8_rs::Playlist::MediaPlaylist(_)));
    }

    #[test]
    fn part_already_available_returns_true() {
        // in-progress segment is MSN 12 with parts 0 and 1 available
        assert!(HlsRouter::playlist_has_part(LL_PLAYLIST.as_bytes(), 12, 0));
        assert!(HlsRouter::playlist_has_part(LL_PLAYLIST.as_bytes(), 12, 1));
    }

    #[test]
    fn earlier_segment_is_always_available() {
        // any part of an already-completed segment is available
        assert!(HlsRouter::playlist_has_part(LL_PLAYLIST.as_bytes(), 11, 99));
        assert!(HlsRouter::playlist_has_part(LL_PLAYLIST.as_bytes(), 10, 0));
    }

    #[test]
    fn future_part_blocks() {
        // part 2 of MSN 12 has not been published yet (only 0,1 exist)
        assert!(!HlsRouter::playlist_has_part(LL_PLAYLIST.as_bytes(), 12, 2));
        // an entire future segment is not available
        assert!(!HlsRouter::playlist_has_part(LL_PLAYLIST.as_bytes(), 13, 0));
    }

    #[test]
    fn invalid_playlist_does_not_unblock() {
        assert!(!HlsRouter::playlist_has_part(b"not a playlist", 0, 0));
    }

    /// Regression: variant/segment path components were joined unchecked; a
    /// percent-encoded ".." or "/" could escape the media base directory.
    #[test]
    fn safe_path_component_rejects_traversal() {
        assert!(HlsRouter::safe_path_component("..").is_err());
        assert!(HlsRouter::safe_path_component(".").is_err());
        assert!(HlsRouter::safe_path_component("").is_err());
        assert!(HlsRouter::safe_path_component("../../etc/passwd").is_err());
        assert!(HlsRouter::safe_path_component("a/b").is_err());
        assert!(HlsRouter::safe_path_component("a\\b").is_err());
        assert!(HlsRouter::safe_path_component("a\0b").is_err());
        assert!(HlsRouter::safe_path_component("1.m4s").is_ok());
        assert!(HlsRouter::safe_path_component("720p").is_ok());
        assert!(
            HlsRouter::safe_path_component("f2a5c3e8-0000-0000-0000-000000000000").is_ok()
        );
    }

    #[test]
    fn get_range_end_is_inclusive_and_bounded() {
        use http_range_header::parse_range_header;
        // bytes=0-99 of a 100 byte file: valid (inclusive end 99)
        let r = parse_range_header("bytes=0-99").unwrap();
        assert!(HlsRouter::get_range(100, r.ranges.first().unwrap()).is_ok());
        // bytes=0-100 of a 100 byte file: end index out of range
        let r = parse_range_header("bytes=0-100").unwrap();
        assert!(HlsRouter::get_range(100, r.ranges.first().unwrap()).is_err());
        // start beyond EOF
        let r = parse_range_header("bytes=100-").unwrap();
        assert!(HlsRouter::get_range(100, r.ranges.first().unwrap()).is_err());
    }

    #[test]
    fn add_token_to_url_appends_correctly() {
        assert_eq!(HlsRouter::add_token_to_url("a/live.m3u8", "tok"), "a/live.m3u8?vt=tok");
        assert_eq!(
            HlsRouter::add_token_to_url("a/live.m3u8?x=1", "tok"),
            "a/live.m3u8?x=1&vt=tok"
        );
    }
}
