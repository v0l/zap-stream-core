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
    ) -> Result<Response, String> {
        let client_ip = Self::get_client_ip(&headers);
        let user_agent = headers.get("user-agent").and_then(|h| h.to_str().ok());

        let token = ViewerTracker::generate_viewer_token(&client_ip, user_agent);

        // Read the playlist file
        let playlist_path = this
            .base_path
            .join(stream.to_string())
            .join(HLS_EGRESS_PATH)
            .join("live.m3u8");
        let playlist_content = tokio::fs::read(playlist_path)
            .await
            .map_err(|e| format!("Failed to read playlist file: {}", e))?;

        // Parse and modify playlist to add viewer token to URLs
        let modified_content = Self::add_viewer_token_to_playlist(&playlist_content, &token)
            .map_err(|e| format!("Failed to add playlist token to playlist: {}", e))?;

        let headers = [(CONTENT_TYPE, "application/vnd.apple.mpegurl")];
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
            .join(&stream_id)
            .join(HLS_EGRESS_PATH)
            .join(variant)
            .join("live.m3u8");

        let stream = FileStream::from_path(&playlist_path)
            .await
            .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
        this.stream_manager.track_viewer(&stream_id, &q.vt).await;
        Ok(stream.into_response())
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
            .join(variant)
            .join(segment);

        let stream = FileStream::from_path(&segment_path)
            .await
            .map_err(|_| (StatusCode::NOT_FOUND, "File not found"))?;
        if let Some(r) = headers.get("range") {
            if let Ok(ranges) = parse_range_header(r.to_str().unwrap()) {
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
                ensure!(i <= file_size, "Range end out of range");
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
    pub vt: String,
}
