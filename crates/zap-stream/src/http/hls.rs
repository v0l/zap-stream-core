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
use std::io::SeekFrom;
use std::ops::Range;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
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
        let modified_content = Self::add_viewer_token_to_playlist(&playlist_content, &token)
            .map_err(|e| {
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
        let mut content = if q.hls_msn.is_some() {
            Self::read_playlist_blocking(&playlist_path, q.hls_msn, q.hls_part)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?
        } else {
            tokio::fs::read(&playlist_path)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?
        };

        // LL-HLS: append EXT-X-RENDITION-REPORT for sibling renditions. Apple's
        // LL-HLS profile requires rendition reports when the multivariant
        // playlist advertises multiple renditions; AVPlayer relies on them to
        // switch levels at the live edge.
        //
        // Only variants referenced by the CURRENT master playlist are reported
        // (the hls dir can contain stale variant dirs from previous sessions),
        // and video variant URIs carry the same viewer token the master
        // playlist rewrite adds, so the reported URLs match the URLs the
        // client actually loaded.
        if content.windows(15).any(|w| w == b"#EXT-X-PART-INF")
            && let Some(hls_dir) = playlist_path.parent().and_then(|p| p.parent())
            && let Some(cur_variant) = playlist_path.parent().and_then(|p| p.file_name())
        {
            let siblings = Self::master_playlist_renditions(
                &hls_dir.join("live.m3u8"),
                &cur_variant.to_string_lossy(),
            )
            .await;
            for (dir, is_media_rendition) in siblings {
                let sib_playlist = hls_dir.join(&dir).join("live.m3u8");
                if let Ok(data) = tokio::fs::read(&sib_playlist).await
                    && let Some((last_msn, last_part)) = Self::playlist_last_msn_part(&data)
                {
                    // EXT-X-MEDIA renditions are loaded without a viewer token;
                    // STREAM-INF variants get ?vt= appended by the master rewrite
                    let uri = match (&q.vt, is_media_rendition) {
                        (Some(vt), false) => {
                            format!("../{}/live.m3u8?vt={}", dir, vt)
                        }
                        _ => format!("../{}/live.m3u8", dir),
                    };
                    let mut line = format!(
                        "#EXT-X-RENDITION-REPORT:URI=\"{}\",LAST-MSN={}",
                        uri, last_msn
                    );
                    if let Some(p) = last_part {
                        line.push_str(&format!(",LAST-PART={}", p));
                    }
                    line.push('\n');
                    content.extend_from_slice(line.as_bytes());
                }
            }
        }

        let headers = [
            (CONTENT_TYPE, Self::PLAYLIST_CONTENT_TYPE),
            // playlists are live and must never be cached by intermediaries
            (axum::http::header::CACHE_CONTROL, "no-cache, no-store"),
        ];
        Ok((headers, content).into_response())
    }

    /// True when the segment following `segment_name` (numeric stem) exists in
    /// the same directory, meaning `segment_name` is complete and will not grow.
    async fn next_segment_exists(segment_path: &std::path::Path, segment_name: &str) -> bool {
        let Some((stem, ext)) = segment_name.rsplit_once('.') else {
            return false;
        };
        let Ok(idx) = stem.parse::<u64>() else {
            return false;
        };
        let Some(dir) = segment_path.parent() else {
            return false;
        };
        tokio::fs::metadata(dir.join(format!("{}.{}", idx + 1, ext)))
            .await
            .is_ok()
    }

    /// Read the master playlist and return the rendition directories it
    /// references, excluding `current`. Returns (dir_name, is_media_rendition)
    /// where is_media_rendition is true for EXT-X-MEDIA entries (audio groups)
    /// and false for EXT-X-STREAM-INF variants.
    async fn master_playlist_renditions(
        master_path: &std::path::Path,
        current: &str,
    ) -> Vec<(String, bool)> {
        let Ok(data) = tokio::fs::read(master_path).await else {
            return vec![];
        };
        let Ok((_, m3u8_rs::Playlist::MasterPlaylist(pl))) = m3u8_rs::parse_playlist(&data) else {
            return vec![];
        };
        // first path component of a variant URI is the rendition directory
        fn dir_of(uri: &str) -> Option<String> {
            let path = uri.split('?').next()?;
            let dir = path.split('/').next()?;
            if dir.is_empty() {
                None
            } else {
                Some(dir.to_string())
            }
        }
        let mut out: Vec<(String, bool)> = Vec::new();
        for alt in &pl.alternatives {
            if let Some(uri) = &alt.uri
                && let Some(d) = dir_of(uri)
                && d != current
                && !out.iter().any(|(e, _)| e == &d)
            {
                out.push((d, true));
            }
        }
        for var in &pl.variants {
            if let Some(d) = dir_of(&var.uri)
                && d != current
                && !out.iter().any(|(e, _)| e == &d)
            {
                out.push((d, false));
            }
        }
        out
    }

    /// Extract (LAST-MSN, LAST-PART) for a rendition report from a media
    /// playlist. Uses the in-progress segment when it has published parts,
    /// otherwise the last completed segment.
    fn playlist_last_msn_part(content: &[u8]) -> Option<(u64, Option<u64>)> {
        let (_, pl) = m3u8_rs::parse_playlist(content).ok()?;
        let pl = match pl {
            m3u8_rs::Playlist::MediaPlaylist(pl) => pl,
            m3u8_rs::Playlist::MasterPlaylist(_) => return None,
        };
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
        if trailing_parts > 0 {
            // in-progress segment with published parts
            Some((pl.media_sequence + full_count, Some(trailing_parts - 1)))
        } else if full_count > 0 {
            Some((pl.media_sequence + full_count - 1, None))
        } else {
            None
        }
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

        loop {
            let content = tokio::fs::read(path).await?;
            if Self::playlist_has_part(&content, want_msn, want_part) || Instant::now() >= deadline
            {
                return Ok(content);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    /// Returns true when the playlist already satisfies a blocking reload
    /// request (RFC 8216bis 6.2.5.2).
    ///
    /// - `_HLS_msn=M&_HLS_part=N`: part N of segment M (or anything later)
    ///   has been published.
    /// - `_HLS_msn=M` alone (`want_part == None`): segment M is *complete*,
    ///   i.e. listed as a full segment. Answering when M has merely started
    ///   (only parts published) is a premature response, which AVPlayer
    ///   reports as "Invalid server blocking reload behavior for low latency".
    fn playlist_has_part(content: &[u8], want_msn: u64, want_part: Option<u64>) -> bool {
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
        // Count full segments and the partial segments that trail the last full
        // one, and remember how many parts each listed full segment had.
        let mut full_count: u64 = 0;
        let mut trailing_parts: u64 = 0;
        let mut parts_of_wanted: Option<u64> = None;
        for seg in &pl.segments {
            match seg {
                m3u8_rs::MediaSegmentType::Full(_) => {
                    if media_sequence + full_count == want_msn {
                        parts_of_wanted = Some(trailing_parts);
                    }
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
            // `want_msn` is a completed full segment. A part index past its
            // final part means part 0 of the following segment (6.2.5.2).
            if let (Some(want_part), Some(n)) = (want_part, parts_of_wanted)
                && n > 0
                && want_part >= n
                && want_msn + 1 == in_progress_msn
            {
                return trailing_parts > 0;
            }
            return true;
        }
        if want_msn == in_progress_msn
            && let Some(want_part) = want_part
        {
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

        if let Some(r) = headers.get("range") {
            let r = r
                .to_str()
                .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid range"))?;
            if let Ok(ranges) = parse_range_header(r) {
                if ranges.ranges.len() > 1 {
                    warn!("Multipart ranges are not supported, fallback to non-range request");
                    let stream = FileStream::from_path(&segment_path)
                        .await
                        .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
                    return Ok(stream.into_response());
                }
                let single = ranges
                    .ranges
                    .into_iter()
                    .next()
                    .ok_or((StatusCode::BAD_REQUEST, "Invalid range"))?;

                // LL-HLS blocking preload hint: hinted requests address bytes at
                // or beyond the current end of the in-progress segment file. The
                // HLS muxer guarantees segment files only ever contain complete,
                // published parts (it owns all file writes and appends at part
                // boundaries), so the file size IS the publication boundary:
                // wait for the file to grow past the requested offset. If the
                // segment has been superseded (next index exists) the hinted
                // part will never appear, so fail fast instead of stalling the
                // client for the whole timeout at every segment rollover.
                if let StartPosition::Index(start) = single.start {
                    use std::time::Duration;
                    use tokio::time::Instant;
                    let deadline = Instant::now() + Duration::from_secs(6);
                    loop {
                        let size = tokio::fs::metadata(&segment_path)
                            .await
                            .map(|m| m.len())
                            .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
                        if size > start {
                            break;
                        }
                        if Self::next_segment_exists(&segment_path, &segment).await {
                            return Err((
                                StatusCode::RANGE_NOT_SATISFIABLE,
                                "Segment is complete, hinted range will not appear",
                            ));
                        }
                        if Instant::now() >= deadline {
                            return Err((
                                StatusCode::RANGE_NOT_SATISFIABLE,
                                "Hinted range did not become available",
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }

                Self::range_response(&segment_path, &single).await
            } else {
                Err((StatusCode::BAD_REQUEST, "Invalid range"))
            }
        } else {
            let stream = FileStream::from_path(&segment_path)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
            Ok(stream.into_response())
        }
    }

    /// Serves a single byte range of a segment file.
    ///
    /// The body must carry only the requested bytes: a part is a byte range
    /// inside a segment file that is still growing, so a whole-file body makes
    /// the player read past the part it asked for.
    async fn range_response(
        segment_path: &std::path::Path,
        header: &SyntacticallyCorrectRange,
    ) -> Result<Response, (StatusCode, &'static str)> {
        let file_size = tokio::fs::metadata(segment_path)
            .await
            .map(|m| m.len())
            .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
        let range = Self::get_range(file_size, header)
            .map_err(|_| (StatusCode::RANGE_NOT_SATISFIABLE, "Invalid range request"))?;

        let mut file = File::open(segment_path)
            .await
            .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Seek failed"))?;
        let len = range.end - range.start + 1;
        let stream = ReaderStream::new(file.take(len));
        Ok(FileStream::new(stream).into_range_response(range.start, range.end, file_size))
    }

    /// Resolve a range header against the current file size (RFC 9110 14.1.2).
    ///
    /// Returns an inclusive `start..end` pair.
    ///
    /// A last-byte-pos at or past the end of the file is NOT an error: the
    /// range is clamped to the last byte. LL-HLS clients rely on this. Players
    /// that follow RFC 8216bis / RFC 8673 address a preload hint without
    /// BYTERANGE-LENGTH as `bytes=<start>-9007199254740991`. Rejecting it with
    /// 416 means the player can never load the live edge.
    ///
    /// Open-ended ranges are served to the end of the file. They used to be
    /// capped at 1 MiB, which cut a high-bitrate part or segment short with a
    /// 206 that looked valid. The body is streamed from disk, so a large range
    /// costs no memory.
    fn get_range(file_size: u64, header: &SyntacticallyCorrectRange) -> Result<Range<u64>> {
        ensure!(file_size > 0, "Empty file has no satisfiable range");
        let last_byte = file_size - 1;
        let range_start = match header.start {
            StartPosition::Index(i) => {
                ensure!(i < file_size, "Range start out of range");
                i
            }
            StartPosition::FromLast(i) => {
                // a zero-length suffix is unsatisfiable
                ensure!(i > 0, "Empty suffix range");
                file_size.saturating_sub(i)
            }
        };
        let range_end = match header.end {
            // HTTP ranges are inclusive; clamp to the last byte of the file
            EndPosition::Index(i) => i.min(last_byte),
            EndPosition::LastByte => last_byte,
        };
        // The body is sliced as end - start + 1, so a reversed range would underflow
        ensure!(range_start <= range_end, "Range start after range end");
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
        assert!(HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            12,
            Some(0)
        ));
        assert!(HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            12,
            Some(1)
        ));
    }

    #[test]
    fn earlier_segment_is_always_available() {
        // any part of an already-completed segment is available
        assert!(HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            11,
            Some(99)
        ));
        assert!(HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            10,
            Some(0)
        ));
    }

    #[test]
    fn future_part_blocks() {
        // part 2 of MSN 12 has not been published yet (only 0,1 exist)
        assert!(!HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            12,
            Some(2)
        ));
        // an entire future segment is not available
        assert!(!HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            13,
            Some(0)
        ));
    }

    /// Regression (AVPlayer "Invalid server blocking reload behavior for low
    /// latency"): `_HLS_msn=M` without `_HLS_part` must wait until segment M is
    /// complete, not merely started. Captured from AVPlayer against prod:
    /// `?_HLS_msn=2532` was answered while 2532 only had part 0.
    #[test]
    fn msn_only_waits_for_complete_segment() {
        // MSN 12 is in progress (parts 0,1) -> not yet complete
        assert!(!HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            12,
            None
        ));
        // MSN 11 is a full segment
        assert!(HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            11,
            None
        ));
        assert!(!HlsRouter::playlist_has_part(
            LL_PLAYLIST.as_bytes(),
            13,
            None
        ));
    }

    /// RFC 8216bis 6.2.5.2: a part index past the final part of a completed
    /// segment is a request for part 0 of the next segment.
    #[test]
    fn part_past_end_of_completed_segment_means_next_part_zero() {
        // MSN 11 completed with 4 parts, MSN 12 not started yet
        let pl = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:10\n#EXT-X-PART-INF:PART-TARGET=0.5\n\
#EXTINF:2,\n10.m4s\n\
#EXT-X-PART:DURATION=0.5,URI=\"11.m4s\",BYTERANGE=\"100@0\",INDEPENDENT=YES\n\
#EXT-X-PART:DURATION=0.5,URI=\"11.m4s\",BYTERANGE=\"100@100\"\n\
#EXT-X-PART:DURATION=0.5,URI=\"11.m4s\",BYTERANGE=\"100@200\"\n\
#EXT-X-PART:DURATION=0.5,URI=\"11.m4s\",BYTERANGE=\"100@300\"\n\
#EXTINF:2,\n11.m4s\n\
#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"12.m4s\",BYTERANGE-START=0\n";
        assert!(HlsRouter::playlist_has_part(pl.as_bytes(), 11, Some(3)));
        // part 4 of 11 == part 0 of 12, which is not published
        assert!(!HlsRouter::playlist_has_part(pl.as_bytes(), 11, Some(4)));
        // once 12 has a part, it is satisfied
        let pl2 = format!(
            "{}#EXT-X-PART:DURATION=0.5,URI=\"12.m4s\",BYTERANGE=\"100@0\",INDEPENDENT=YES\n",
            pl.trim_end_matches("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"12.m4s\",BYTERANGE-START=0\n")
        );
        assert!(HlsRouter::playlist_has_part(pl2.as_bytes(), 11, Some(4)));
    }

    #[test]
    fn invalid_playlist_does_not_unblock() {
        assert!(!HlsRouter::playlist_has_part(b"not a playlist", 0, Some(0)));
    }

    #[test]
    fn last_msn_part_uses_in_progress_segment() {
        // MSN 12 in progress with parts 0,1 -> LAST-MSN=12, LAST-PART=1
        assert_eq!(
            HlsRouter::playlist_last_msn_part(LL_PLAYLIST.as_bytes()),
            Some((12, Some(1)))
        );
    }

    #[test]
    fn last_msn_part_without_trailing_parts() {
        let pl = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2,\n10.m4s\n#EXTINF:2,\n11.m4s\n";
        assert_eq!(
            HlsRouter::playlist_last_msn_part(pl.as_bytes()),
            Some((11, None))
        );
    }

    #[test]
    fn last_msn_part_invalid_playlist() {
        assert_eq!(HlsRouter::playlist_last_msn_part(b"junk"), None);
    }

    /// Regression: hinted requests for a completed segment (rollover) must
    /// fail fast instead of blocking for the full timeout.
    #[tokio::test]
    async fn next_segment_exists_detects_rollover() {
        let dir = tempfile::tempdir().unwrap();
        let p12 = dir.path().join("12.m4s");
        std::fs::write(&p12, b"x").unwrap();
        // no 13.m4s yet -> still in progress
        assert!(!HlsRouter::next_segment_exists(&p12, "12.m4s").await);
        std::fs::write(dir.path().join("13.m4s"), b"y").unwrap();
        assert!(HlsRouter::next_segment_exists(&p12, "12.m4s").await);
        // non-numeric names never match
        assert!(!HlsRouter::next_segment_exists(&p12, "init.mp4").await);
    }

    /// Regression: a range response must carry only the requested bytes. A
    /// part is a byte range inside a segment file that keeps growing, so a
    /// whole-file body makes the player read past the part it asked for.
    #[tokio::test]
    async fn range_response_body_is_sliced() {
        use axum::body::to_bytes;

        let dir = tempfile::tempdir().unwrap();
        let seg = dir.path().join("12.m4s");
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        std::fs::write(&seg, &data).unwrap();

        let header = parse_range_header("bytes=100-199").unwrap();
        let resp = HlsRouter::range_response(&seg, &header.ranges[0])
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_RANGE)
                .unwrap(),
            "bytes 100-199/1000"
        );

        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.len(), 100);
        assert_eq!(&body[..], &data[100..200]);

        // A one-byte range is legal and must not be widened to the whole file.
        let header = parse_range_header("bytes=0-0").unwrap();
        let resp = HlsRouter::range_response(&seg, &header.ranges[0])
            .await
            .unwrap();
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_RANGE)
                .unwrap(),
            "bytes 0-0/1000"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], &data[..1]);
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
        assert!(HlsRouter::safe_path_component("f2a5c3e8-0000-0000-0000-000000000000").is_ok());
    }

    #[test]
    fn get_range_end_is_inclusive_and_bounded() {
        use http_range_header::parse_range_header;
        // bytes=0-99 of a 100 byte file: valid (inclusive end 99)
        let r = parse_range_header("bytes=0-99").unwrap();
        assert_eq!(
            HlsRouter::get_range(100, r.ranges.first().unwrap()).unwrap(),
            0..99
        );
        // bytes=0-100 of a 100 byte file: end past EOF is clamped (RFC 9110 14.1.2)
        let r = parse_range_header("bytes=0-100").unwrap();
        assert_eq!(
            HlsRouter::get_range(100, r.ranges.first().unwrap()).unwrap(),
            0..99
        );
        // start beyond EOF
        let r = parse_range_header("bytes=100-").unwrap();
        assert!(HlsRouter::get_range(100, r.ranges.first().unwrap()).is_err());
        // reversed range: rejected rather than underflowing the body length
        let r = parse_range_header("bytes=500-100").unwrap();
        assert!(HlsRouter::get_range(1000, r.ranges.first().unwrap()).is_err());
        // suffix ranges
        let r = parse_range_header("bytes=-10").unwrap();
        assert_eq!(
            HlsRouter::get_range(100, r.ranges.first().unwrap()).unwrap(),
            90..99
        );
        // nothing is satisfiable in an empty file
        let r = parse_range_header("bytes=0-").unwrap();
        assert!(HlsRouter::get_range(0, r.ranges.first().unwrap()).is_err());
    }

    /// Regression (iOS/AVPlayer playback): LL-HLS clients request a preload
    /// hint that has no BYTERANGE-LENGTH with the RFC 8673 open-ended form
    /// `bytes=<start>-9007199254740991`. The server returned 416, so the live
    /// edge could never load.
    #[test]
    fn get_range_accepts_rfc8673_live_range() {
        use http_range_header::parse_range_header;
        let r = parse_range_header("bytes=0-9007199254740991").unwrap();
        assert_eq!(
            HlsRouter::get_range(1_954_416, r.ranges.first().unwrap()).unwrap(),
            0..1_954_415
        );
        let r = parse_range_header("bytes=618488-9007199254740991").unwrap();
        assert_eq!(
            HlsRouter::get_range(981_258, r.ranges.first().unwrap()).unwrap(),
            618_488..981_257
        );
    }

    /// Regression: an open-ended range was capped at 1 MiB, so a request for a
    /// 1080p segment or a large part got a truncated 206 that looked valid.
    #[test]
    fn get_range_open_ended_is_not_truncated() {
        use http_range_header::parse_range_header;
        let size = 1_954_416u64; // real 1080p@8Mbps 2s segment
        let r = parse_range_header("bytes=0-").unwrap();
        assert_eq!(
            HlsRouter::get_range(size, r.ranges.first().unwrap()).unwrap(),
            0..size - 1
        );
    }

    /// The body for an over-long end must match the clamped Content-Range.
    #[tokio::test]
    async fn range_response_clamps_end_past_eof() {
        use axum::body::to_bytes;
        let dir = tempfile::tempdir().unwrap();
        let seg = dir.path().join("7.m4s");
        let data: Vec<u8> = (0..=255u8).cycle().take(3 * 1024 * 1024).collect();
        std::fs::write(&seg, &data).unwrap();

        let header = parse_range_header("bytes=100-9007199254740991").unwrap();
        let resp = HlsRouter::range_response(&seg, &header.ranges[0])
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_RANGE)
                .unwrap(),
            &format!("bytes 100-{}/{}", data.len() - 1, data.len())
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], &data[100..]);
    }

    /// End-to-end through the real router, replaying the request sequences
    /// AVPlayer (macOS 26.5, AVFoundation) sent to production and failed on.
    mod avplayer_replay {
        use super::*;
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use std::time::Duration;
        use tower::ServiceExt;

        const STREAM: &str = "537a365c-f1ec-44ac-af10-22d14a7319fb";
        const VAR: &str = "16f75f1a";

        /// Variant playlist: 10 complete with 4 parts, 11 not started yet.
        fn rollover_playlist() -> String {
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:9\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXT-X-PART-INF:PART-TARGET=0.5\n\
#EXT-X-SERVER-CONTROL:PART-HOLD-BACK=1.500,CAN-BLOCK-RELOAD=YES\n\
#EXTINF:2,\n9.m4s\n\
#EXT-X-PART:DURATION=0.5,URI=\"10.m4s\",BYTERANGE=\"100@0\",INDEPENDENT=YES\n\
#EXT-X-PART:DURATION=0.5,URI=\"10.m4s\",BYTERANGE=\"100@100\"\n\
#EXT-X-PART:DURATION=0.5,URI=\"10.m4s\",BYTERANGE=\"100@200\"\n\
#EXT-X-PART:DURATION=0.5,URI=\"10.m4s\",BYTERANGE=\"100@300\"\n\
#EXTINF:2,\n10.m4s\n\
#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"11.m4s\",BYTERANGE-START=0\n"
                .to_string()
        }

        fn setup() -> (tempfile::TempDir, std::path::PathBuf, Router) {
            let dir = tempfile::tempdir().unwrap();
            let var_dir = dir.path().join(STREAM).join(HLS_EGRESS_PATH).join(VAR);
            std::fs::create_dir_all(&var_dir).unwrap();
            std::fs::write(var_dir.join("live.m3u8"), rollover_playlist()).unwrap();
            std::fs::write(var_dir.join("10.m4s"), vec![7u8; 400]).unwrap();
            let router = HlsRouter::new(dir.path(), StreamManager::new("test".into()));
            (dir, var_dir, router)
        }

        async fn get(router: &Router, uri: &str, range: Option<&str>) -> (StatusCode, Vec<u8>) {
            let mut req = Request::get(uri);
            if let Some(r) = range {
                req = req.header("range", r);
            }
            let resp = router
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            (
                status,
                to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
        }

        /// Captured: `?_HLS_msn=2636&_HLS_part=4` (2636 had parts 0..3) was
        /// answered in 30ms with a playlist where 2637 had no parts. AVPlayer
        /// logged "Invalid server blocking reload behavior for low latency"
        /// (-15410) and dropped out of low-latency mode.
        #[tokio::test]
        async fn part_past_end_blocks_until_next_segment_part_zero() {
            let (_d, var_dir, router) = setup();
            let uri = format!("/{STREAM}/hls/{VAR}/live.m3u8?_HLS_msn=10&_HLS_part=4");

            let publish = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let pl = rollover_playlist().replace(
                    "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"11.m4s\",BYTERANGE-START=0\n",
                    "#EXT-X-PART:DURATION=0.5,URI=\"11.m4s\",BYTERANGE=\"100@0\",INDEPENDENT=YES\n\
#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"11.m4s\",BYTERANGE-START=100\n",
                );
                std::fs::write(var_dir.join("live.m3u8"), pl).unwrap();
            });

            let t = std::time::Instant::now();
            let (status, body) = get(&router, &uri, None).await;
            publish.await.unwrap();
            assert_eq!(status, StatusCode::OK);
            assert!(
                t.elapsed() >= Duration::from_millis(250),
                "answered before part 0 of MSN 11 existed"
            );
            assert!(
                String::from_utf8(body)
                    .unwrap()
                    .contains("URI=\"11.m4s\",BYTERANGE=\"100@0\"")
            );
        }

        /// `_HLS_msn=M` alone must wait for M to be complete.
        #[tokio::test]
        async fn msn_only_blocks_until_segment_complete() {
            let (_d, var_dir, router) = setup();
            let uri = format!("/{STREAM}/hls/{VAR}/live.m3u8?_HLS_msn=11");
            let publish = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let pl = rollover_playlist().replace(
                    "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"11.m4s\",BYTERANGE-START=0\n",
                    "#EXTINF:2,\n11.m4s\n",
                );
                std::fs::write(var_dir.join("live.m3u8"), pl).unwrap();
            });
            let t = std::time::Instant::now();
            let (status, body) = get(&router, &uri, None).await;
            publish.await.unwrap();
            assert_eq!(status, StatusCode::OK);
            assert!(t.elapsed() >= Duration::from_millis(250));
            assert!(String::from_utf8(body).unwrap().contains("\n11.m4s\n"));
        }

        /// Captured: `Range: bytes=36659-9007199254740991` -> 416, and
        /// AVPlayer failed the item with -11850 "server is not correctly
        /// configured".
        #[tokio::test]
        async fn rfc8673_range_on_published_bytes_is_206() {
            let (_d, _v, router) = setup();
            let uri = format!("/{STREAM}/hls/{VAR}/10.m4s");
            let (status, body) = get(&router, &uri, Some("bytes=100-9007199254740991")).await;
            assert_eq!(status, StatusCode::PARTIAL_CONTENT);
            assert_eq!(body.len(), 300);
        }

        /// Hinted RFC 8673 range on the in-progress file: waits for the part,
        /// then returns only the bytes that exist.
        #[tokio::test]
        async fn rfc8673_hinted_range_blocks_then_serves() {
            let (_d, var_dir, router) = setup();
            std::fs::write(var_dir.join("11.m4s"), b"").unwrap();
            let uri = format!("/{STREAM}/hls/{VAR}/11.m4s");
            let w = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                std::fs::write(var_dir.join("11.m4s"), vec![1u8; 150]).unwrap();
            });
            let (status, body) = get(&router, &uri, Some("bytes=0-9007199254740991")).await;
            w.await.unwrap();
            assert_eq!(status, StatusCode::PARTIAL_CONTENT);
            assert_eq!(body.len(), 150);
        }
    }

    #[test]
    fn add_token_to_url_appends_correctly() {
        assert_eq!(
            HlsRouter::add_token_to_url("a/live.m3u8", "tok"),
            "a/live.m3u8?vt=tok"
        );
        assert_eq!(
            HlsRouter::add_token_to_url("a/live.m3u8?x=1", "tok"),
            "a/live.m3u8?x=1&vt=tok"
        );
    }
}
