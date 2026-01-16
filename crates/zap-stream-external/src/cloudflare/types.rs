use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct ApiResponse<T> {
    #[serde(default)]
    pub errors: Vec<ApiError>,
    pub result: T,
    pub success: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiError {
    pub code: i32,
    pub message: Option<String>,
}

/// Details about a Cloudflare Live Input
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LiveInput {
    pub uid: String,
    pub rtmps: RtmpsEndpoint,
    pub rtmps_playback: Option<RtmpsEndpoint>,
    pub srt: Option<SrtEndpoint>,
    pub srt_playback: Option<SrtEndpoint>,
    #[serde(rename = "webRTC")]
    pub webrtc: Option<WebRtcEndpoint>,
    #[serde(rename = "webRTCPlayback")]
    pub webrtc_playback: Option<WebRtcEndpoint>,
    pub status: Option<serde_json::Value>,
    pub created: String,
    pub modified: Option<String>,
    pub meta: Option<serde_json::Value>,
    pub recording: Option<RecordingSettings>,
    pub delete_recording_after_days: Option<u32>,
}

/// RTMPS endpoint details
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RtmpsEndpoint {
    pub url: String,
    pub stream_key: String,
}

/// SRT endpoint details
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SrtEndpoint {
    pub url: String,
    pub stream_id: String,
    pub passphrase: String,
}

/// WebRTC endpoint details
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcEndpoint {
    pub url: String,
}

/// Recording settings for a Live Input
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSettings {
    pub mode: String,
    pub timeout_seconds: Option<u32>,
    #[serde(rename = "requireSignedURLs")]
    pub require_signed_urls: Option<bool>,
    pub allowed_origins: Option<Vec<String>>,
    pub hide_live_viewer_count: Option<bool>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveInputOutput {
    pub enabled: bool,
    pub stream_key: String,
    pub uid: String,
    pub url: String,
}

/// A Cloudflare Video Asset
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VideoAsset {
    pub uid: String,
    pub playback: Playback,
    pub live_input: String,
    pub status: Option<serde_json::Value>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// Playback URLs for a Video Asset
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Playback {
    pub hls: String,
    pub dash: String,
}

/// Cloudflare Live Input webhook payload
/// Based on: https://developers.cloudflare.com/stream/stream-live/webhooks/
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LiveInputWebhook {
    pub name: String,
    pub text: String,
    pub data: LiveInputWebhookData,
    pub ts: i64,
}

/// Live Input webhook data containing event information
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LiveInputWebhookData {
    pub notification_name: String,
    #[serde(rename = "input_id")]
    pub input_id: String,
    #[serde(rename = "event_type")]
    pub event_type: String,
    #[serde(rename = "updated_at")]
    pub updated_at: String,
}

/// Webhook configuration result
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WebhookResult {
    pub notification_url: String,
    pub modified: String,
    pub secret: String,
}

/// Cloudflare Video Asset webhook payload
/// Sent when a recording is ready after a live stream ends
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VideoAssetWebhook {
    pub uid: String,
    pub thumbnail: String,
    pub duration: f32,
    pub playback: Playback,
    pub live_input: String,
    pub status: VideoAssetStatus,
}

/// Video Asset status information
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VideoAssetStatus {
    pub state: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum WebhookPayload {
    LiveInput(LiveInputWebhook),
    VideoAsset(VideoAssetWebhook),
}
