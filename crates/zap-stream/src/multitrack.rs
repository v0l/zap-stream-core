use anyhow::Context;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_d2q, av_get_pix_fmt, av_get_sample_fmt, avcodec_profile_name,
};
use ffmpeg_rs_raw::{cstr, free_cstr, rstr};
use nostr_sdk::serde_json;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::sync::Arc;
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;
use zap_stream_core::endpoint::{EndpointConfigEngine, EndpointConfigurator, VariantType};
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_core::listen::ListenerEndpoint;
use zap_stream_core::overseer::{IngressInfo, IngressStream, StreamType};
use zap_stream_core::variant::VariantStream;
use zap_stream_core::{map_codec_id, recommended_bitrate};

#[derive(Clone, Serialize, Deserialize)]
pub struct MultiTrackEngineConfig {
    pub public_url: String,
    pub dashboard_url: Option<String>,
}

#[derive(Clone)]
pub struct MultiTrackEngine {
    config: MultiTrackEngineConfig,
    endpoint_config: Arc<dyn EndpointConfigurator>,
}

impl MultiTrackEngine {
    const DASHBOARD_LINK: &'static str = "<a href=\"https://zap.stream/dashboard\">zap.stream</a>";

    pub fn new(
        config: MultiTrackEngineConfig,
        endpoint_config: Arc<dyn EndpointConfigurator>,
    ) -> Self {
        Self {
            config,
            endpoint_config,
        }
    }

    pub async fn get_multi_track_config(
        &self,
        req: MultiTrackConfigRequest,
    ) -> anyhow::Result<MultiTrackConfigResponse> {
        let conn = ConnectionInfo {
            id: Uuid::new_v4(),
            endpoint: "multi-track-config".to_string(),
            ip_addr: "".to_string(),
            app_name: "live".to_string(),
            key: req.authentication.clone(),
        };

        let egress_config = match self.endpoint_config.get_egress(&conn).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(MultiTrackConfigResponse::status_error(format!(
                    "Stream key is invalid please visit {} to find your stream key. ({})",
                    self.config
                        .dashboard_url
                        .as_ref()
                        .map(|s| s.as_str())
                        .unwrap_or(Self::DASHBOARD_LINK),
                    e
                )));
            }
        };
        let caps = self.endpoint_config.get_capabilities(&conn).await?;

        let canvas = req
            .preferences
            .canvases
            .iter()
            .next()
            .context("no canvases")?;

        // TODO: pick best ingest codec for best egress copy mapping
        let ingest_video_codec = req
            .client
            .supported_codecs
            .first()
            .map(|s| s.as_str())
            .unwrap_or("h264");
        let ingest_audio_codec = "aac";
        let pix_fmt = "yuv420p";
        let sample_fmt = "fltp";

        let ingest_video_codec_id = map_codec_id(ingest_video_codec)
            .context(format!("No video codec for {}", ingest_video_codec))?;
        let ingest_audio_codec_id = map_codec_id(ingest_audio_codec)
            .context(format!("No video codec for {}", ingest_audio_codec))?;

        // create a fake ingress info using the supported params sent in the request
        // this ingress info can be used by the pipeline endpoint config system to create
        // what the ideal output variants should be
        let canvas_fps = canvas.framerate.numerator as f32 / canvas.framerate.denominator as f32;
        let pseudo_ingress = IngressInfo {
            bitrate: req.preferences.maximum_aggregate_bitrate.unwrap_or(0) as _,
            streams: vec![
                IngressStream {
                    index: 0,
                    stream_type: StreamType::Video,
                    codec: ingest_video_codec_id as _,
                    format: unsafe {
                        let str = cstr!(pix_fmt);
                        let ret: i32 = av_get_pix_fmt(str) as _;
                        if ret == -1 {
                            return Ok(MultiTrackConfigResponse::status_error(format!(
                                "Could not find pixel format {}",
                                pix_fmt
                            )));
                        }
                        free_cstr!(str);
                        ret
                    } as _,
                    profile: 0,
                    level: 0,
                    color_space: 0,
                    color_range: 0,
                    width: canvas.width as _,
                    height: canvas.height as _,
                    fps: canvas_fps,
                    sample_rate: 0,
                    bitrate: if let Some(c) = caps.iter().find_map(|c| match c {
                        VariantType::Variant { bitrate, height }
                            if *height as u32 == canvas.height =>
                        {
                            Some(*bitrate)
                        }
                        _ => None,
                    }) {
                        c as _
                    } else {
                        recommended_bitrate(
                            ingest_video_codec,
                            canvas.width as u64 * canvas.height as u64,
                            canvas_fps,
                        ) as _
                    },
                    channels: 0,
                    language: "".to_string(),
                },
                IngressStream {
                    index: 1,
                    stream_type: StreamType::Audio,
                    codec: ingest_audio_codec_id as _,
                    format: unsafe {
                        let str = cstr!(sample_fmt);
                        let ret: i32 = av_get_sample_fmt(str) as _;
                        free_cstr!(str);
                        if ret == -1 {
                            return Ok(MultiTrackConfigResponse::status_error(format!(
                                "Could not find sample format {}",
                                sample_fmt
                            )));
                        }
                        ret
                    } as _,
                    profile: 0,
                    level: 0,
                    color_space: 0,
                    color_range: 0,
                    width: 0,
                    height: 0,
                    fps: 0.0,
                    sample_rate: req.preferences.audio_samples_per_sec as _,
                    bitrate: 320_000,
                    channels: req.preferences.audio_channels as _,
                    language: "".to_string(),
                },
            ],
        };
        let encoder_config = match EndpointConfigEngine::get_variants_from_endpoint(
            &pseudo_ingress,
            &caps,
            &egress_config,
        ) {
            Ok(c) => c,
            Err(e) => {
                return Ok(MultiTrackConfigResponse::status_error(format!(
                    "Failed to configure stream <pre>{}</pre>",
                    e
                )));
            }
        };
        info!("{}", encoder_config);
        let max_audio = encoder_config
            .variants
            .iter()
            .max_by_key(|a| match a {
                VariantStream::Audio(a) | VariantStream::CopyAudio(a) => a.bitrate,
                _ => 0,
            })
            .map(|a| match a {
                VariantStream::Audio(a) | VariantStream::CopyAudio(a) => a,
                _ => unreachable!(),
            });

        let ingress = self.endpoint_config.get_ingress().await?;
        let public_url: Url = self.config.public_url.parse()?;
        Ok(MultiTrackConfigResponse {
            ingest_endpoints: ingress
                .iter()
                .filter_map(|c| {
                    Some(MTIngestEndpoint {
                        protocol: match c {
                            ListenerEndpoint::RTMP { .. } => "RTMP".to_string(),
                            ListenerEndpoint::SRT { .. } => "SRT".to_string(),
                            _ => return None,
                        },
                        url_template: c
                            .to_public_url(&public_url.host()?.to_string(), "live/{stream_key}")?,
                        authentication: None,
                    })
                })
                .collect(),
            encoder_configurations: encoder_config
                .variants
                .iter()
                .filter_map(|c| match c {
                    VariantStream::Video(v) | VariantStream::CopyVideo(v) => {
                        // https://docs.aws.amazon.com/ivs/latest/BroadcastSWIntegAPIReference/structures-VideoTrackSettings.html
                        let mut settings_obj = serde_json::Map::new();
                        let fps_frac = unsafe { av_d2q(v.fps as _, 90_000) };
                        if v.profile != 0
                            && let Some(codec_id) = map_codec_id(&v.codec)
                        {
                            let profile_name = unsafe {
                                let np = avcodec_profile_name(codec_id, v.profile as _);
                                if np.is_null() {
                                    None
                                } else {
                                    Some(rstr!(np).to_string())
                                }
                            };
                            if let Some(pp) = profile_name {
                                settings_obj.insert("profile".to_owned(), pp.into());
                            }
                        }
                        if let Some(t) = &v.tune {
                            settings_obj.insert("tune".to_owned(), t.to_string().into());
                        }
                        if v.level != 0 {
                            settings_obj.insert("level".to_owned(), v.level.into());
                        }
                        if v.bitrate != 0 {
                            settings_obj.insert("bitrate".to_owned(), (v.bitrate / 1000).into());
                        }
                        if v.gop != 0 {
                            settings_obj
                                .insert("keyint_sec".to_owned(), (v.gop as f32 / v.fps).into());
                        }
                        if v.max_b_frames != 0 {
                            settings_obj.insert("bf".to_owned(), v.max_b_frames.into());
                        }
                        settings_obj.insert("rate_control".to_string(), "CBR".to_string().into());
                        let Some(encoder) = req.find_best_encoder_for_codec(&v.codec) else {
                            warn!("Could not find encoder for codec {}", v.codec);
                            return None;
                        };
                        Some(MTVideoEncoderConfig {
                            r#type: encoder.name,
                            width: v.width as _,
                            height: v.height as _,
                            framerate: Some(MTFramerate {
                                numerator: fps_frac.num as _,
                                denominator: fps_frac.den as _,
                            }),
                            gpu_scale_type: if v.width as u32 != canvas.width
                                || v.height as u32 != canvas.height
                            {
                                Some(MTObsScale::BiCubic)
                            } else {
                                Some(MTObsScale::Disable)
                            },
                            colorspace: match v.color_space.to_lowercase().as_str() {
                                "bt709" => Some(MTVideoColorspace::BT709),
                                "bt2001" => Some(MTVideoColorspace::BT2100PQ),
                                _ => Some(MTVideoColorspace::BT709),
                            },
                            range: match v.color_range.to_lowercase().as_str() {
                                "full" => Some(MTVideoRange::Full),
                                "partial" => Some(MTVideoRange::Partial),
                                _ => Some(MTVideoRange::Default),
                            },
                            format: if encoder.implementation.is_gpu() {
                                Some(MTVideoFormat::NV12)
                            } else {
                                Some(MTVideoFormat::I420)
                            },
                            settings: settings_obj.into(),
                            canvas_index: 0, // TODO: map to canvas from req
                        })
                    }
                    _ => None,
                })
                .collect(),
            audio_configurations: MTAudioConfig {
                live: if let Some(audio) = max_audio
                    && let Some(enc) = req.find_best_encoder_for_codec(&audio.codec)
                {
                    vec![MTAudioEncoderConfig {
                        codec: enc.name,
                        track_id: 0,
                        channels: audio.channels as _,
                        settings: serde_json::json!({
                            "bitrate": audio.bitrate / 1000
                        }),
                    }]
                } else {
                    Vec::new()
                },
                vod: None,
            },
            ..Default::default()
        })
        //Ok(ingest_pick.into())
    }
}

// https://github.com/obsproject/obs-studio/blob/master/frontend/utility/models/multitrack-video.hpp
#[derive(Clone, Serialize, Deserialize)]
pub struct MultiTrackConfigRequest {
    pub authentication: String,
    pub capabilities: MTCapabilities,
    pub client: MTClient,
    pub preferences: MTPreferences,
    pub schema_version: String,
    pub service: String,
}

impl MultiTrackConfigRequest {
    /// Find the best encoder for a given codec from a list of supported encoder IDs.
    /// Returns the best encoder sorted by priority (hardware VRAM > hardware RAM > software > unknown).
    pub fn find_best_encoder_for_codec(&self, codec: &str) -> Option<ObsEncoderType> {
        let mut encoders: Vec<ObsEncoderType> = self
            .client
            .supported_encoders
            .iter()
            .filter_map(|id| ObsEncoderType::try_from(id.as_str()).ok())
            .filter(|enc| enc.codec == codec)
            .collect();

        encoders.sort();
        if let Some(e) = encoders.into_iter().next() {
            Some(e)
        } else {
            self.get_fallback_encoder(codec)
        }
    }

    /// Using the CPU/GPU info create an encoder
    pub fn get_fallback_encoder(&self, codec: &str) -> Option<ObsEncoderType> {
        // shortcut for audio codec
        match codec {
            "aac" => {
                return Some(ObsEncoderType {
                    name: "ffmpeg_aac".to_string(),
                    api: ObsEncoderApi::OBS,
                    codec: "aac".to_string(),
                    implementation: ObsEncoderImplementation::Software {
                        name: "ffmpeg".to_string(),
                    },
                });
            }
            _ => {}
        }

        if let Some(gpu) = self.capabilities.gpu.last() {
            match gpu.vendor_id {
                0x8086 => {
                    // INTEL
                    if self
                        .capabilities
                        .system
                        .version
                        .to_lowercase()
                        .contains("linux")
                    {
                        Some(ObsEncoderType {
                            name: match codec {
                                "h264" => "ffmpeg_vaapi_tex".to_string(),
                                "hevc" | "av1" => {
                                    format!("{}_ffmpeg_vaapi_tex", codec.to_lowercase())
                                }
                                _ => return None,
                            },
                            api: ObsEncoderApi::FFMPEG,
                            codec: codec.to_string(),
                            implementation: ObsEncoderImplementation::VAAPI {
                                transfer_type: ObsGpuEncoderMemoryArea::VRAM,
                            },
                        })
                    } else {
                        Some(ObsEncoderType {
                            name: match codec {
                                "h264" => "obs_qsv11_v2".to_string(),
                                "hevc" | "av1" => format!("obs_qsv11_{}", codec.to_lowercase()),
                                _ => return None,
                            },
                            api: ObsEncoderApi::OBS,
                            codec: codec.to_string(),
                            implementation: ObsEncoderImplementation::QSV,
                        })
                    }
                }
                0x12d2 | 0x10de => {
                    // NVIDIA
                    Some(ObsEncoderType {
                        name: format!("obs_nvenc_{}_tex", codec.to_lowercase()),
                        api: ObsEncoderApi::OBS,
                        codec: codec.to_string(),
                        implementation: ObsEncoderImplementation::NVENC {
                            transfer_type: ObsGpuEncoderMemoryArea::VRAM,
                        },
                    })
                }
                _ => None,
            }
        } else {
            None
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTCapabilities {
    pub cpu: MTCpu,
    pub memory: MTMemory,
    pub gaming_features: Option<MTGamingFeatures>,
    pub system: MTSystem,
    #[serde(default)]
    pub gpu: Vec<MTGpu>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTCpu {
    pub logical_cores: i32,
    pub physical_cores: i32,
    pub speed: Option<u32>,
    pub name: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTGpu {
    pub model: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: u64,
    pub shared_system_memory: u64,
    pub driver_version: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTGamingFeatures {
    #[serde(default)]
    pub game_bar_enabled: bool,
    #[serde(default)]
    pub game_dvr_allowed: bool,
    #[serde(default)]
    pub game_dvr_enabled: bool,
    #[serde(default)]
    pub game_dvr_bg_recording: bool,
    #[serde(default)]
    pub game_mode_enabled: bool,
    #[serde(default)]
    pub hags_enabled: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTMemory {
    pub free: u64,
    pub total: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTSystem {
    pub version: String,
    pub name: String,
    pub build: i32,
    pub release: String,
    pub revision: String,
    pub bits: i32,
    #[serde(default)]
    pub arm: bool,
    #[serde(default)]
    pub arm_emulation: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTClient {
    pub name: String,
    pub supported_codecs: Vec<String>,
    /// https://github.com/obsproject/obs-studio/pull/12867
    #[serde(default)]
    pub supported_encoders: Vec<String>,
    pub version: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTPreferences {
    pub maximum_aggregate_bitrate: Option<u64>,
    pub maximum_video_tracks: Option<u64>,
    pub vod_track_audio: bool,
    pub composition_gpu_index: Option<u32>,
    pub audio_samples_per_sec: u32,
    pub audio_channels: u32,
    pub audio_max_buffering_ms: u32,
    pub audio_fixed_buffering: bool,
    pub canvases: Vec<MTCanvas>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTCanvas {
    pub width: u32,
    pub height: u32,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub framerate: MTFramerate,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTFramerate {
    pub denominator: i64,
    pub numerator: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MultiTrackConfigResponse {
    pub meta: MTMeta,
    pub status: Option<MTStatus>,
    pub ingest_endpoints: Vec<MTIngestEndpoint>,
    pub encoder_configurations: Vec<MTVideoEncoderConfig>,
    pub audio_configurations: MTAudioConfig,
}

impl MultiTrackConfigResponse {
    pub fn status_error(msg: String) -> Self {
        MultiTrackConfigResponse {
            status: Some(MTStatus {
                result: MTStatusCode::Error,
                html_en_us: Some(msg),
            }),
            ..Default::default()
        }
    }
}

impl Default for MultiTrackConfigResponse {
    fn default() -> Self {
        MultiTrackConfigResponse {
            meta: MTMeta {
                service: "zap-stream-core".to_string(),
                schema_version: "2025-01-25".to_string(),
                config_id: "".to_string(),
            },
            status: None,
            ingest_endpoints: vec![],
            encoder_configurations: vec![],
            audio_configurations: MTAudioConfig {
                live: vec![],
                vod: None,
            },
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MTStatusCode {
    Unknown,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTStatus {
    pub result: MTStatusCode,
    pub html_en_us: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTMeta {
    pub service: String,
    pub schema_version: String,
    pub config_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTIngestEndpoint {
    pub protocol: String,
    pub url_template: String,
    pub authentication: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTVideoEncoderConfig {
    pub r#type: String,
    pub width: u32,
    pub height: u32,
    pub framerate: Option<MTFramerate>,
    pub gpu_scale_type: Option<MTObsScale>,
    pub colorspace: Option<MTVideoColorspace>,
    pub range: Option<MTVideoRange>,
    pub format: Option<MTVideoFormat>,
    pub settings: serde_json::Value,
    pub canvas_index: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum MTObsScale {
    #[serde(rename = "OBS_SCALE_DISABLE")]
    Disable,
    #[serde(rename = "OBS_SCALE_POINT")]
    Point,
    #[serde(rename = "OBS_SCALE_BICUBIC")]
    BiCubic,
    #[serde(rename = "OBS_SCALE_BILINEAR")]
    Bilinear,
    #[serde(rename = "OBS_SCALE_LANCZOS")]
    Lanczos,
    #[serde(rename = "OBS_SCALE_AREA")]
    Area,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MTVideoColorspace {
    #[serde(rename = "VIDEO_CS_DEFAULT")]
    Default,
    #[serde(rename = "VIDEO_CS_601")]
    BT601,
    #[serde(rename = "VIDEO_CS_709")]
    BT709,
    #[serde(rename = "VIDEO_CS_SRGB")]
    SRGB,
    #[serde(rename = "VIDEO_CS_2100_PQ")]
    BT2100PQ,
    #[serde(rename = "VIDEO_CS_2100_HLG")]
    BT2100HLG,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum MTVideoFormat {
    #[serde(rename = "VIDEO_FORMAT_NONE")]
    None,
    #[serde(rename = "VIDEO_FORMAT_I420")]
    I420,
    #[serde(rename = "VIDEO_FORMAT_NV12")]
    NV12,
    #[serde(rename = "VIDEO_FORMAT_BGRA")]
    BGRA,
    #[serde(rename = "VIDEO_FORMAT_I444")]
    I444,
    #[serde(rename = "VIDEO_FORMAT_I010")]
    I010,
    #[serde(rename = "VIDEO_FORMAT_P010")]
    P010,
    #[serde(rename = "VIDEO_FORMAT_P216")]
    P216,
    #[serde(rename = "VIDEO_FORMAT_P416")]
    P416,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum MTVideoRange {
    #[serde(rename = "VIDEO_RANGE_DEFAULT")]
    Default,
    #[serde(rename = "VIDEO_RANGE_PARTIAL")]
    Partial,
    #[serde(rename = "VIDEO_RANGE_FULL")]
    Full,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTAudioEncoderConfig {
    pub codec: String,
    pub track_id: u32,
    pub channels: u32,
    pub settings: serde_json::Value,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MTAudioConfig {
    pub live: Vec<MTAudioEncoderConfig>,
    pub vod: Option<Vec<MTAudioEncoderConfig>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObsEncoderType {
    /// The name of the encoder in OBS
    pub name: String,

    /// The package which accesses the encoder (OBS/FFMPEG etc.)
    pub api: ObsEncoderApi,

    /// The codec name
    pub codec: String,

    /// The package implementing the codec
    pub implementation: ObsEncoderImplementation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ObsEncoderApi {
    /// Could not determine the implementation
    Unknown,
    /// OBS directly calls the encoder
    OBS,
    /// OBS uses FFMPEG to call the encoder
    FFMPEG,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ObsEncoderImplementation {
    /// Could not determine the implementation
    Unknown,
    /// Implemented by software encoder package
    Software { name: String },
    /// Intel Quick-Sync video encoder
    QSV,
    /// Nvidia encoder
    NVENC {
        transfer_type: ObsGpuEncoderMemoryArea,
    },
    /// Linux Video Acceleration API
    VAAPI {
        transfer_type: ObsGpuEncoderMemoryArea,
    },
}

impl ObsEncoderImplementation {
    pub fn is_gpu(&self) -> bool {
        match self {
            ObsEncoderImplementation::QSV => true,
            ObsEncoderImplementation::VAAPI { transfer_type }
                if *transfer_type == ObsGpuEncoderMemoryArea::VRAM =>
            {
                true
            }
            ObsEncoderImplementation::NVENC { transfer_type }
                if *transfer_type == ObsGpuEncoderMemoryArea::VRAM =>
            {
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Eq)]
pub enum ObsGpuEncoderMemoryArea {
    Unknown,
    /// Encoded frames stay in vRAM
    VRAM,
    /// Encoded frames go via RAM
    RAM,
}

impl ObsGpuEncoderMemoryArea {
    /// Returns a priority score for sorting (higher is better)
    /// VRAM > RAM > Unknown
    fn priority(&self) -> u8 {
        match self {
            ObsGpuEncoderMemoryArea::VRAM => 2,
            ObsGpuEncoderMemoryArea::RAM => 1,
            ObsGpuEncoderMemoryArea::Unknown => 0,
        }
    }
}

impl ObsEncoderImplementation {
    /// Returns a priority score for sorting (higher is better)
    /// Hardware encoders (NVENC/VAAPI/QSV) > Software > Unknown
    /// Within GPU encoders with transfer_type, VRAM is preferred over RAM
    fn priority(&self) -> u8 {
        match self {
            ObsEncoderImplementation::NVENC { transfer_type } => 100 + transfer_type.priority(),
            ObsEncoderImplementation::VAAPI { transfer_type } => 100 + transfer_type.priority(),
            ObsEncoderImplementation::QSV => 100,
            ObsEncoderImplementation::Software { .. } => 50,
            ObsEncoderImplementation::Unknown => 0,
        }
    }
}

impl Eq for ObsEncoderType {}

impl PartialOrd for ObsEncoderType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ObsEncoderType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher priority implementations should come first (reverse order)
        other
            .implementation
            .priority()
            .cmp(&self.implementation.priority())
    }
}

impl TryFrom<&str> for ObsEncoderType {
    type Error = ();

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        if value.contains("ffmpeg") {
            // Handle codec_ffmpeg_* patterns (e.g., hevc_ffmpeg_vaapi_tex, av1_ffmpeg_vaapi_tex)
            if let Some((codec, rest)) = value.split_once("_ffmpeg_") {
                let transfer_type = if value.contains("_tex") {
                    ObsGpuEncoderMemoryArea::VRAM
                } else {
                    ObsGpuEncoderMemoryArea::RAM
                };
                let implementation = if rest.starts_with("vaapi") {
                    ObsEncoderImplementation::VAAPI { transfer_type }
                } else if rest.starts_with("nvenc") {
                    ObsEncoderImplementation::NVENC { transfer_type }
                } else {
                    ObsEncoderImplementation::Unknown
                };
                Ok(ObsEncoderType {
                    name: value.to_string(),
                    api: ObsEncoderApi::FFMPEG,
                    codec: codec.to_string(),
                    implementation,
                })
            } else {
                let (_, codec) = value.split_once('_').ok_or(())?;
                // base VAAPI type only supports h264
                if codec.starts_with("vaapi") {
                    let transfer_type = if value.contains("_tex") {
                        ObsGpuEncoderMemoryArea::VRAM
                    } else {
                        ObsGpuEncoderMemoryArea::RAM
                    };
                    Ok(ObsEncoderType {
                        name: value.to_string(),
                        api: ObsEncoderApi::FFMPEG,
                        codec: "h264".to_string(),
                        implementation: ObsEncoderImplementation::VAAPI { transfer_type },
                    })
                } else if codec.ends_with("_av1") {
                    // AV1 software encoders like svt_av1, aom_av1
                    let impl_name = codec.strip_suffix("_av1").unwrap();
                    Ok(ObsEncoderType {
                        name: value.to_string(),
                        api: ObsEncoderApi::FFMPEG,
                        codec: "av1".to_string(),
                        implementation: ObsEncoderImplementation::Software {
                            name: impl_name.to_string(),
                        },
                    })
                } else {
                    Ok(ObsEncoderType {
                        name: value.to_string(),
                        api: ObsEncoderApi::FFMPEG,
                        codec: codec.to_string(),
                        implementation: ObsEncoderImplementation::Unknown,
                    })
                }
            }
        } else if value.contains("obs_") {
            let mut split = value.split("_");
            split.next(); // skip obs_
            let codec = split.next().ok_or(())?;
            let (codec, hw) = match codec {
                "qsv11" => (
                    match split.next() {
                        Some("hevc") => "hevc",
                        _ => "h264",
                    },
                    ObsEncoderImplementation::QSV,
                ),
                "nvenc" => {
                    let transfer_type = if value.ends_with("_tex") {
                        ObsGpuEncoderMemoryArea::VRAM
                    } else {
                        ObsGpuEncoderMemoryArea::RAM
                    };
                    (
                        split.next().unwrap_or("h264"),
                        ObsEncoderImplementation::NVENC { transfer_type },
                    )
                }
                _ => (codec, ObsEncoderImplementation::Unknown),
            };
            Ok(ObsEncoderType {
                name: value.to_string(),
                api: ObsEncoderApi::OBS,
                codec: codec.to_string(),
                implementation: hw,
            })
        } else {
            Ok(ObsEncoderType {
                name: value.to_string(),
                api: ObsEncoderApi::Unknown,
                codec: "".to_string(),
                implementation: ObsEncoderImplementation::Unknown,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zap_stream_core::egress::EgressType;
    use zap_stream_core::endpoint::VariantType;
    use zap_stream_core::mux::SegmentType;

    #[test]
    fn test_obs_encoder_ids() {
        // QSV HEVC encoders
        let enc = ObsEncoderType::try_from("obs_qsv11_hevc_soft").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "hevc");
        assert_eq!(enc.implementation, ObsEncoderImplementation::QSV);

        let enc = ObsEncoderType::try_from("obs_qsv11_hevc").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "hevc");
        assert_eq!(enc.implementation, ObsEncoderImplementation::QSV);

        // QSV H264 encoders
        let enc = ObsEncoderType::try_from("obs_qsv11_soft_v2").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "h264");
        assert_eq!(enc.implementation, ObsEncoderImplementation::QSV);

        let enc = ObsEncoderType::try_from("obs_qsv11_v2").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "h264");
        assert_eq!(enc.implementation, ObsEncoderImplementation::QSV);

        // OBS x264 software encoder
        let enc = ObsEncoderType::try_from("obs_x264").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "x264");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        // FFMPEG audio encoders
        let enc = ObsEncoderType::try_from("ffmpeg_opus").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "opus");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        let enc = ObsEncoderType::try_from("ffmpeg_aac").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "aac");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        let enc = ObsEncoderType::try_from("ffmpeg_pcm_s16le").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "pcm_s16le");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        let enc = ObsEncoderType::try_from("ffmpeg_pcm_s24le").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "pcm_s24le");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        let enc = ObsEncoderType::try_from("ffmpeg_pcm_f32le").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "pcm_f32le");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        let enc = ObsEncoderType::try_from("ffmpeg_alac").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "alac");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        let enc = ObsEncoderType::try_from("ffmpeg_flac").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "flac");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);

        // FFMPEG AV1 software encoders
        let enc = ObsEncoderType::try_from("ffmpeg_svt_av1").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "av1");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::Software {
                name: "svt".to_string()
            }
        );

        let enc = ObsEncoderType::try_from("ffmpeg_aom_av1").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "av1");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::Software {
                name: "aom".to_string()
            }
        );

        // FFMPEG VAAPI H264 encoders
        let enc = ObsEncoderType::try_from("ffmpeg_vaapi").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "h264");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            }
        );

        let enc = ObsEncoderType::try_from("ffmpeg_vaapi_tex").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "h264");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        );

        // FFMPEG VAAPI HEVC encoders
        let enc = ObsEncoderType::try_from("hevc_ffmpeg_vaapi_tex").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "hevc");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        );

        let enc = ObsEncoderType::try_from("hevc_ffmpeg_vaapi").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "hevc");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            }
        );

        // FFMPEG VAAPI AV1 encoders (codec at start)
        let enc = ObsEncoderType::try_from("av1_ffmpeg_vaapi_tex").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "av1");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        );

        let enc = ObsEncoderType::try_from("av1_ffmpeg_vaapi").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::FFMPEG);
        assert_eq!(enc.codec, "av1");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            }
        );

        // OBS NVENC encoders with _tex suffix (VRAM)
        let enc = ObsEncoderType::try_from("obs_nvenc_av1_tex").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "av1");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        );

        let enc = ObsEncoderType::try_from("obs_nvenc_hevc_tex").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "hevc");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        );

        let enc = ObsEncoderType::try_from("obs_nvenc_h264_tex").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "h264");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        );

        // OBS NVENC encoders without _tex suffix (RAM)
        let enc = ObsEncoderType::try_from("obs_nvenc_av1").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "av1");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            }
        );

        let enc = ObsEncoderType::try_from("obs_nvenc_hevc").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "hevc");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            }
        );

        let enc = ObsEncoderType::try_from("obs_nvenc_h264").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::OBS);
        assert_eq!(enc.codec, "h264");
        assert_eq!(
            enc.implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            }
        );

        // Unknown encoder
        let enc = ObsEncoderType::try_from("some_unknown_encoder").unwrap();
        assert_eq!(enc.api, ObsEncoderApi::Unknown);
        assert_eq!(enc.codec, "");
        assert_eq!(enc.implementation, ObsEncoderImplementation::Unknown);
    }

    #[test]
    fn test_obs_encoder_sorting() {
        let mut encoders = vec![
            ObsEncoderType::try_from("ffmpeg_aac").unwrap(), // Unknown impl
            ObsEncoderType::try_from("ffmpeg_svt_av1").unwrap(), // Software
            ObsEncoderType::try_from("obs_nvenc_h264").unwrap(), // NVENC RAM
            ObsEncoderType::try_from("obs_nvenc_h264_tex").unwrap(), // NVENC VRAM
            ObsEncoderType::try_from("ffmpeg_vaapi").unwrap(), // VAAPI RAM
            ObsEncoderType::try_from("ffmpeg_vaapi_tex").unwrap(), // VAAPI VRAM
            ObsEncoderType::try_from("obs_qsv11_hevc").unwrap(), // QSV
        ];

        encoders.sort();

        // Hardware VRAM encoders should come first
        assert!(matches!(
            encoders[0].implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            } | ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        ));
        assert!(matches!(
            encoders[1].implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            } | ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::VRAM
            }
        ));

        // Then hardware RAM encoders
        assert!(matches!(
            encoders[2].implementation,
            ObsEncoderImplementation::NVENC {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            } | ObsEncoderImplementation::VAAPI {
                transfer_type: ObsGpuEncoderMemoryArea::RAM
            } | ObsEncoderImplementation::QSV
        ));

        // Software should be after hardware
        assert!(matches!(
            encoders[5].implementation,
            ObsEncoderImplementation::Software { .. }
        ));

        // Unknown should be last
        assert!(matches!(
            encoders[6].implementation,
            ObsEncoderImplementation::Unknown
        ));
    }

    #[tokio::test]
    async fn test_get_multi_track_config_linux_nvidia() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .try_init()
            .ok();
        let cfg = DummyEndpointConfigurator {
            ingress: vec![ListenerEndpoint::RTMP {
                endpoint: "rtmp://0.0.0.0:1935".to_string(),
            }],
            caps: vec![
                VariantType::SourceVariant,
                VariantType::Variant {
                    height: 1080,
                    bitrate: 6_000_000,
                },
                VariantType::Variant {
                    height: 720,
                    bitrate: 3_000_000,
                },
                VariantType::Variant {
                    height: 480,
                    bitrate: 800_000,
                },
                VariantType::DVR { height: 720 },
            ],
            egress: vec![
                EgressType::HLS {
                    id: Uuid::new_v4(),
                    segment_length: 2.0,
                    segment_type: SegmentType::FMP4,
                },
                EgressType::Recorder {
                    id: Uuid::new_v4(),
                    height: 720,
                },
            ],
        };
        let engine = MultiTrackEngine::new(
            MultiTrackEngineConfig {
                public_url: "https://localhost".to_string(),
                dashboard_url: None,
            },
            Arc::new(cfg),
        );

        let req = MultiTrackConfigRequest {
            authentication: "test".to_string(),
            capabilities: MTCapabilities {
                cpu: MTCpu {
                    logical_cores: 20,
                    physical_cores: 10,
                    speed: Some(3_000),
                    name: Some("Intel Core i9-14900K".to_string()),
                },
                memory: MTMemory {
                    free: 54961823744,
                    total: 67148804096,
                },
                gaming_features: None,
                system: MTSystem {
                    version: "linux".to_string(),
                    name: "Ubuntu".to_string(),
                    build: 0,
                    release: "24.04".to_string(),
                    revision: "".to_string(),
                    bits: 64,
                    arm: false,
                    arm_emulation: false,
                },
                gpu: vec![MTGpu {
                    model: "GeForce RTX 5090".to_string(),
                    vendor_id: 4318,
                    device_id: 0,
                    dedicated_video_memory: 69_000_000_000,
                    shared_system_memory: 0,
                    driver_version: Some("NVIDIA 535.274.2".to_string()),
                }],
            },
            client: MTClient {
                name: "obs-studio".to_string(),
                supported_codecs: vec!["h264".to_string(), "h265".to_string(), "av1".to_string()],
                supported_encoders: vec![],
                version: "32.0.2".to_string(),
            },
            preferences: MTPreferences {
                maximum_aggregate_bitrate: Some(12_000_000),
                maximum_video_tracks: None,
                vod_track_audio: false,
                composition_gpu_index: Some(0),
                audio_samples_per_sec: 48_000,
                audio_channels: 2,
                audio_max_buffering_ms: 960,
                audio_fixed_buffering: false,
                canvases: vec![MTCanvas {
                    width: 2560,
                    height: 1440,
                    canvas_width: 2560,
                    canvas_height: 1440,
                    framerate: MTFramerate {
                        denominator: 1,
                        numerator: 30,
                    },
                }],
            },
            schema_version: "2025-01-25".to_string(),
            service: "IVS".to_string(),
        };

        let rsp = engine.get_multi_track_config(req).await.expect("config");

        let ep = rsp.ingest_endpoints.first().expect("endpoint");
        assert_eq!(ep.protocol, "RTMP");
        assert_eq!(ep.url_template, "rtmp://localhost:1935/live/{stream_key}");

        let audio = rsp.audio_configurations.live.first().expect("audio");
        assert!(rsp.audio_configurations.vod.is_none());
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.codec, "ffmpeg_aac");
        assert_eq!(audio.track_id, 0);
        assert_eq!(audio.settings.get("bitrate"), Some(&json!(192)));

        let v1440 = rsp
            .encoder_configurations
            .iter()
            .find(|c| c.height == 1440)
            .expect("encoder-1440");
        let v1080 = rsp
            .encoder_configurations
            .iter()
            .find(|c| c.height == 1080)
            .expect("encoder-1080");
        let v720 = rsp
            .encoder_configurations
            .iter()
            .find(|c| c.height == 720)
            .expect("encoder-720");
        let v480 = rsp
            .encoder_configurations
            .iter()
            .find(|c| c.height == 480)
            .expect("encoder-480");

        assert_eq!(v1440.width, 2560);
        assert_eq!(v1440.height, 1440);
        assert_eq!(v1440.canvas_index, 0);
        assert_eq!(v1440.format, Some(MTVideoFormat::NV12));
        assert_eq!(
            v1440.settings.get("bitrate"),
            Some(&json!(recommended_bitrate("h264", 2560 * 1440, 30.0) / 1000))
        );

        assert_eq!(v1080.width, 1920);
        assert_eq!(v1080.height, 1080);
        assert_eq!(v1080.canvas_index, 0);
        assert_eq!(v1080.format, Some(MTVideoFormat::NV12));
        assert_eq!(v1080.settings.get("bitrate"), Some(&json!(6000)));

        assert_eq!(v720.width, 1280);
        assert_eq!(v720.height, 720);
        assert_eq!(v720.canvas_index, 0);
        assert_eq!(v720.format, Some(MTVideoFormat::NV12));
        assert_eq!(v720.settings.get("bitrate"), Some(&json!(3000)));

        assert_eq!(v480.width, 854);
        assert_eq!(v480.height, 480);
        assert_eq!(v480.canvas_index, 0);
        assert_eq!(v480.format, Some(MTVideoFormat::NV12));
        assert_eq!(v480.settings.get("bitrate"), Some(&json!(800)));
    }

    struct DummyEndpointConfigurator {
        ingress: Vec<ListenerEndpoint>,
        caps: Vec<VariantType>,
        egress: Vec<EgressType>,
    }

    #[async_trait::async_trait]
    impl EndpointConfigurator for DummyEndpointConfigurator {
        async fn get_capabilities(
            &self,
            conn: &ConnectionInfo,
        ) -> anyhow::Result<Vec<VariantType>> {
            Ok(self.caps.clone())
        }

        async fn get_ingress(&self) -> anyhow::Result<Vec<ListenerEndpoint>> {
            Ok(self.ingress.clone())
        }

        async fn get_egress(&self, conn: &ConnectionInfo) -> anyhow::Result<Vec<EgressType>> {
            Ok(self.egress.clone())
        }

        #[cfg(feature = "moq")]
        async fn get_moq_origin(
            &self,
        ) -> anyhow::Result<zap_stream_core::hang::moq_lite::OriginProducer> {
            todo!()
        }
    }
}
