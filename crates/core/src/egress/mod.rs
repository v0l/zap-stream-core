use crate::overseer::{IngressStream, StreamType};
use crate::variant::VariantStream;
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVStream;
use ffmpeg_rs_raw::{AvPacketRef, Encoder};
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
            StreamType::Video => {
                Some(EgressEncoderConfig {
                    codec: "h264".to_string(),
                    codec_params: vec![
                        EncoderParam::Preset {
                            name: "veryfast".to_string(),
                        },
                        EncoderParam::Tune {
                            name: "zerolatency".to_string(),
                        },
                        EncoderParam::GOPSize {
                            size: stream.fps as u32 * 2,
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
                    ]
                    .into(),
                    stream_type: StreamType::Video,
                })
            }
            StreamType::Audio => Some(EgressEncoderConfig {
                codec: "aac".to_string(),
                codec_params: vec![
                    EncoderParam::SampleFormat {
                        name: "fltp".to_string(),
                    },
                    EncoderParam::SampleRate {
                        size: if stream.sample_rate == 44100 || stream.sample_rate == 48000 {
                            stream.sample_rate as _
                        } else {
                            48_000 // Default to 48kHz if non-standard sample rate
                        },
                    },
                    EncoderParam::AudioChannels {
                        count: if stream.channels < 3 {
                            stream.channels as _
                        } else {
                            2
                        },
                    },
                ]
                .into(),
                stream_type: StreamType::Audio,
            }),
            _ => None,
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
