use serde::Deserialize;

/// Response from Cloudflare Live Input creation/retrieval API
#[derive(Debug, Deserialize)]
pub struct LiveInputResponse {
    pub result: LiveInput,
    pub success: bool,
}

/// Details about a Cloudflare Live Input
#[derive(Debug, Deserialize)]
pub struct LiveInput {
    pub uid: String,
    pub rtmps: RtmpsEndpoint,
    #[serde(rename = "rtmpsPlayback")]
    pub rtmps_playback: Option<RtmpsEndpoint>,
    pub srt: Option<SrtEndpoint>,
    #[serde(rename = "srtPlayback")]
    pub srt_playback: Option<SrtEndpoint>,
    #[serde(rename = "webRTC")]
    pub webrtc: Option<WebRtcEndpoint>,
    #[serde(rename = "webRTCPlayback")]
    pub webrtc_playback: Option<WebRtcEndpoint>,
    pub status: Option<String>,
    pub created: String,
    pub modified: Option<String>,
    pub meta: Option<serde_json::Value>,
    pub recording: Option<RecordingSettings>,
    #[serde(rename = "deleteRecordingAfterDays")]
    pub delete_recording_after_days: Option<u32>,
}

/// RTMPS endpoint details
#[derive(Debug, Deserialize, Clone)]
pub struct RtmpsEndpoint {
    pub url: String,
    #[serde(rename = "streamKey")]
    pub stream_key: String,
}

/// SRT endpoint details
#[derive(Debug, Deserialize)]
pub struct SrtEndpoint {
    pub url: String,
    #[serde(rename = "streamId")]
    pub stream_id: String,
    pub passphrase: String,
}

/// WebRTC endpoint details
#[derive(Debug, Deserialize)]
pub struct WebRtcEndpoint {
    pub url: String,
}

/// Recording settings for a Live Input
#[derive(Debug, Deserialize)]
pub struct RecordingSettings {
    pub mode: String,
    #[serde(rename = "timeoutSeconds")]
    pub timeout_seconds: Option<u32>,
    #[serde(rename = "requireSignedURLs")]
    pub require_signed_urls: Option<bool>,
    #[serde(rename = "allowedOrigins")]
    pub allowed_origins: Option<Vec<String>>,
    #[serde(rename = "hideLiveViewerCount")]
    pub hide_live_viewer_count: Option<bool>,
}

/// Response from Cloudflare Videos API (filtered by liveInput)
#[derive(Debug, Deserialize)]
pub struct VideoAssetsResponse {
    pub result: Vec<VideoAsset>,
    pub success: bool,
}

/// A Cloudflare Video Asset
#[derive(Debug, Deserialize)]
pub struct VideoAsset {
    pub uid: String,
    pub playback: Playback,
    #[serde(rename = "liveInput")]
    pub live_input: String,
    pub status: Option<serde_json::Value>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// Playback URLs for a Video Asset
#[derive(Debug, Deserialize)]
pub struct Playback {
    pub hls: String,
    pub dash: String,
}

/// Cloudflare Stream Live webhook payload
#[derive(Debug, Deserialize)]
pub struct CloudflareWebhookPayload {
    pub data: CloudflareWebhookData,
}

/// Webhook data containing event information
#[derive(Debug, Deserialize)]
pub struct CloudflareWebhookData {
    #[serde(rename = "event_type")]
    pub event_type: String,
    #[serde(rename = "input_id")]
    pub input_id: String,
    #[serde(rename = "updated_at")]
    pub updated_at: String,
}

/// Response from webhook registration API
#[derive(Debug, Deserialize)]
pub struct WebhookResponse {
    pub result: WebhookResult,
    pub success: bool,
}

/// Webhook configuration result
#[derive(Debug, Deserialize)]
pub struct WebhookResult {
    #[serde(rename = "notificationUrl")]
    pub notification_url: String,
    pub modified: String,
    pub secret: String,
}
