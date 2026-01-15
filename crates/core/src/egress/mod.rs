use crate::ingress::{IngressStream, StreamType};
use crate::mux::SegmentType;
use crate::variant::{VariantGroup, VariantStream};
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AVStream, av_d2q};
use ffmpeg_rs_raw::{AvPacketRef, Encoder};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use uuid::Uuid;

#[cfg(feature = "egress-hls")]
pub mod hls;
#[cfg(feature = "egress-moq")]
pub mod moq;
pub mod muxer_egress;

pub trait Egress: Send {
    fn process_pkt(&mut self, packet: AvPacketRef, variant: &Uuid) -> Result<EgressResult>;
    fn reset(&mut self) -> Result<EgressResult>;
}

#[derive(Debug, Clone)]
pub enum EgressResult {
    /// Nothing to report
    None,
    /// Egress created/deleted some segments
    Segments {
        created: Vec<EgressSegment>,
        deleted: Vec<EgressSegment>,
    },
}

/// Basic details of new segment created by a muxer
#[derive(Debug, Clone)]
pub struct EgressSegment {
    /// The id of the variant (video or audio)
    pub variant: Uuid,
    /// Segment index
    pub idx: u64,
    /// Duration in seconds
    pub duration: f32,
    /// Path on disk to the segment file
    pub path: PathBuf,
    /// SHA-256 hash of the file
    pub sha256: [u8; 32],
}

pub enum EncoderOrSourceStream<'a> {
    Encoder(&'a Encoder),
    SourceStream(*mut AVStream),
}

pub struct EncoderVariantGroup<'a> {
    pub id: Uuid,
    pub streams: Vec<EncoderVariant<'a>>,
}

pub struct EncoderVariant<'a> {
    pub variant: &'a VariantStream,
    pub stream: EncoderOrSourceStream<'a>,
}

/// Codec information which is used to configure the transcoding pipeline
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct EgressEncoderConfig {
    /// Codec name
    pub codec: String,
    /// Codec params
    pub codec_params: EncoderParams,
    /// The ingress stream type
    pub stream_type: StreamType,
}

impl EgressEncoderConfig {
    /// Default H.264 codec params used by HLS/Recorder/RTMP-Egress/MoQ
    /// The goal is to reduce the number of transocded variants, if each egress had distinct options
    /// you would need to transcode multiple times
    pub fn default_h264(stream: &IngressStream) -> Option<Self> {
        match stream.stream_type {
            StreamType::Video => Some(Self::default_video_h264(stream.fps)),
            StreamType::Audio => Some(Self::default_audio_h264(
                stream.sample_rate as _,
                stream.channels,
            )),
            _ => None,
        }
    }

    pub fn default_video_h264(fps: f32) -> EgressEncoderConfig {
        let frac = unsafe { av_d2q(fps as _, 90_000) };
        EgressEncoderConfig {
            codec: "h264".to_string(),
            codec_params: vec![
                EncoderParam::Preset {
                    name: "veryfast".to_string(),
                },
                EncoderParam::Tune {
                    name: "zerolatency".to_string(),
                },
                EncoderParam::GOPSize {
                    size: (fps * 2.0) as _,
                },
                EncoderParam::MaxBFrames { size: 3 },
                EncoderParam::Level {
                    id: 51, // H.264 High 5.1 (4K)
                },
                EncoderParam::Profile {
                    id: 77, // AV_PROFILE_H264_MAIN
                },
                EncoderParam::PixelFormat {
                    name: "yuv420p".to_string(),
                },
                EncoderParam::ColorRange {
                    name: "full".to_string(),
                },
                EncoderParam::ColorSpace {
                    name: "bt709".to_string(),
                },
                EncoderParam::Framerate {
                    num: frac.num as _,
                    den: frac.den as _,
                },
            ]
            .into(),
            stream_type: StreamType::Video,
        }
    }

    pub fn default_audio_h264(sample_rate: u32, channels: u8) -> EgressEncoderConfig {
        EgressEncoderConfig {
            codec: "aac".to_string(),
            codec_params: vec![
                EncoderParam::SampleFormat {
                    name: "fltp".to_string(),
                },
                EncoderParam::SampleRate {
                    size: if sample_rate == 44100 || sample_rate == 48000 {
                        sample_rate
                    } else {
                        48_000 // Default to 48kHz if non-standard sample rate
                    },
                },
                EncoderParam::AudioChannels {
                    count: if channels < 3 { channels as _ } else { 2 },
                },
            ]
            .into(),
            stream_type: StreamType::Audio,
        }
    }
}

/// Generic encoder params, some or all apply to different encoders
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum EncoderParam {
    /// Video width
    Width { value: u16 },
    /// Video height
    Height { value: u16 },
    /// Media average bitrate
    Bitrate { value: u64 },
    /// Video FPS
    Framerate { num: u32, den: u32 },
    /// Encoder preset
    Preset { name: String },
    /// Encoder tune param
    Tune { name: String },
    /// Encoder color space name
    ColorSpace { name: String },
    /// Encoder color range name
    ColorRange { name: String },
    /// Number of frames in a group
    GOPSize { size: u32 },
    /// Max number of B frames
    MaxBFrames { size: u32 },
    /// Codec profile (h264)
    Profile { id: u32 },
    /// Codec level (h264)
    Level { id: u32 },
    /// Video pixel format name
    PixelFormat { name: String },
    /// Audio sample format name
    SampleFormat { name: String },
    /// Audio samples per second
    SampleRate { size: u32 },
    /// Audio channel count
    AudioChannels { count: u32 },
}

impl EncoderParam {
    pub fn eq_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// Wrapper struct over [Vec<EncoderParam>] to prevent duplicate params
#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct EncoderParams(Vec<EncoderParam>);

impl EncoderParams {
    pub fn add_param(&mut self, param: EncoderParam) {
        if let Some(existing) = self.0.iter_mut().find(|v| v.eq_type(&param)) {
            *existing = param;
        } else {
            self.0.push(param);
        }
    }

    pub fn add_params(&mut self, param: Vec<EncoderParam>) {
        for param in param {
            self.add_param(param);
        }
    }

    pub fn with_param(mut self, param: EncoderParam) -> Self {
        self.add_param(param);
        self
    }

    pub fn with_params(mut self, params: Vec<EncoderParam>) -> Self {
        self.add_params(params);
        self
    }

    pub fn extend(&mut self, other: Self) {
        self.add_params(other.0);
    }
}

impl Deref for EncoderParams {
    type Target = Vec<EncoderParam>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for EncoderParams {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<EncoderParam>> for EncoderParams {
    fn from(v: Vec<EncoderParam>) -> Self {
        EncoderParams(v)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum EgressType {
    /// HLS output egress
    HLS {
        /// Unique id of this egress
        id: Uuid,
        /// Segment length in seconds
        segment_length: f32,
        /// Segment type
        segment_type: SegmentType,
    },
    /// Record streams to local disk
    Recorder {
        /// Unique id of this egress
        id: Uuid,
        /// Desired video size height in pixels
        height: u16,
    },
    /// Forward streams to another RTMP server
    RTMPForwarder {
        /// Unique id of this egress
        id: Uuid,
        /// Destination RTMP url
        destination: String,
    },
    /// Media over Quic egress
    Moq {
        /// Unique id of this egress
        id: Uuid,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EgressConfig {
    pub kind: EgressType,
    /// Groups of variants
    pub variants: Vec<VariantGroup>,
}

impl EgressType {
    pub fn id(&self) -> Uuid {
        match self {
            EgressType::HLS { id, .. } => *id,
            EgressType::Recorder { id, .. } => *id,
            EgressType::RTMPForwarder { id, .. } => *id,
            EgressType::Moq { id } => *id,
        }
    }

    /// Get the encoder params this egress needs to process encoded packets
    pub fn get_encoder_params(
        &self,
        stream: &IngressStream,
        input_params: &EncoderParams,
    ) -> Option<EgressEncoderConfig> {
        if matches!(self, EgressType::Recorder { .. }) {
            // Recorder doesn't expect any encoder params because it only re-muxes existing variants
            return None;
        }

        let mut p = EgressEncoderConfig::default_h264(stream)?;
        p.codec_params.extend(input_params.clone());
        if matches!(self, EgressType::HLS { .. }) && stream.stream_type == StreamType::Audio {
            // for HLS force the audio bitrate to always be 192kb/s
            p.codec_params
                .add_param(EncoderParam::Bitrate { value: 192_000 });
        }
        Some(p)
    }
}
