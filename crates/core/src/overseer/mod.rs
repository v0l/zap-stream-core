use crate::ingress::{ConnectionInfo, EndpointStats};

use crate::egress::EgressSegment;
use crate::pipeline::runner::PipelineStats;
use crate::pipeline::{EgressType, PipelineConfig};
use anyhow::{Result, bail};
use async_trait::async_trait;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_get_pix_fmt_name, av_get_sample_fmt_name, avcodec_get_name,
};
use ffmpeg_rs_raw::rstr;
use std::cmp::PartialEq;
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use std::path::PathBuf;
use uuid::Uuid;

/// A copy of [ffmpeg_rs_raw::DemuxerInfo] without internal ptr
#[derive(PartialEq, Clone)]
pub struct IngressInfo {
    pub bitrate: usize,
    pub streams: Vec<IngressStream>,
}

/// A copy of [ffmpeg_rs_raw::StreamInfo] without ptr
#[derive(PartialEq, Clone, Debug, Default)]
pub struct IngressStream {
    pub index: usize,
    pub stream_type: StreamType,
    /// FFMPEG codec ID
    pub codec: isize,
    /// FFMPEG sample/pixel format ID
    pub format: isize,
    pub profile: isize,
    pub level: isize,
    pub color_space: isize,
    pub color_range: isize,
    pub width: usize,
    pub height: usize,
    pub fps: f32,
    pub sample_rate: usize,
    pub bitrate: usize,
    pub channels: u8,
    pub language: String,
}

impl IngressStream {
    /// Get the name of the codec from the FFMPEG codec ID
    pub fn codec_name(&self) -> Result<String> {
        unsafe {
            let codec = avcodec_get_name(transmute(self.codec as i32));
            if codec.is_null() {
                bail!("Codec not found {}", self.codec);
            }
            Ok(rstr!(codec).to_string())
        }
    }

    pub fn pixel_format_name(&self) -> Result<String> {
        if self.stream_type != StreamType::Video {
            bail!("Ingress stream type not Video");
        }
        unsafe {
            let name = av_get_pix_fmt_name(transmute(self.format as i32));
            if name.is_null() {
                bail!("Pixel format not found {}", self.format);
            }
            Ok(rstr!(name).to_string())
        }
    }

    pub fn sample_format_name(&self) -> Result<String> {
        if self.stream_type != StreamType::Audio {
            bail!("Ingress stream type not Audio");
        }
        unsafe {
            let name = av_get_sample_fmt_name(transmute(self.format as i32));
            if name.is_null() {
                bail!("Sample format not found {}", self.format);
            }
            Ok(rstr!(name).to_string())
        }
    }
}

impl Display for IngressStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let codec_name = self.codec_name().unwrap_or_else(|_| "unknown".to_string());
        match self.stream_type {
            StreamType::Video => {
                let pix_fmt = self
                    .pixel_format_name()
                    .unwrap_or_else(|_| "unknown".to_string());
                write!(
                    f,
                    "#{} Video: {}x{} @ {:.2}fps, {} ({}), {}kbps",
                    self.index,
                    self.width,
                    self.height,
                    self.fps,
                    codec_name,
                    pix_fmt,
                    self.bitrate / 1000
                )
            }
            StreamType::Audio => {
                let sample_fmt = self
                    .sample_format_name()
                    .unwrap_or_else(|_| "unknown".to_string());
                write!(
                    f,
                    "#{} Audio: {}ch @ {}Hz, {} ({}), {}kbps",
                    self.index,
                    self.channels,
                    self.sample_rate,
                    codec_name,
                    sample_fmt,
                    self.bitrate / 1000
                )?;
                if !self.language.is_empty() {
                    write!(f, ", lang={}", self.language)?;
                }
                Ok(())
            }
            StreamType::Subtitle => {
                write!(f, "#{} Subtitle: {}", self.index, codec_name)?;
                if !self.language.is_empty() {
                    write!(f, ", lang={}", self.language)?;
                }
                Ok(())
            }
            StreamType::Unknown => write!(f, "#{} Unknown", self.index),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Default)]
pub enum StreamType {
    #[default]
    Unknown,
    Video,
    Audio,
    Subtitle,
}

#[derive(Debug)]
pub enum StatsType {
    Ingress(EndpointStats),
    Pipeline(PipelineStats),
    Egress(EndpointStats),
}

#[derive(Debug, Clone)]
pub enum ConnectResult {
    Allow {
        /// Enable dumping stream data to disk for debugging purposes
        enable_stream_dump: bool,
        /// Replace the stream/pipeline id
        stream_id_override: Option<Uuid>,
    },
    Deny {
        reason: String,
    },
}

#[async_trait]
/// The control process that oversees streaming operations
pub trait Overseer: Send + Sync {
    /// Check all streams
    async fn check_streams(&self) -> Result<()>;

    /// Authorize connection for user
    async fn connect(&self, connection_info: &ConnectionInfo) -> Result<ConnectResult>;

    /// Set up a new streaming pipeline
    async fn start_stream(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig>;

    /// A new segment(s) (HLS etc.) was generated for a stream variant
    ///
    /// This handler is usually used for distribution / billing
    async fn on_segments(
        &self,
        pipeline_id: &Uuid,
        added: &Vec<EgressSegment>,
        deleted: &Vec<EgressSegment>,
    ) -> Result<()>;

    /// At a regular interval, pipeline will emit one of the frames for processing as a
    /// thumbnail
    async fn on_thumbnail(
        &self,
        pipeline_id: &Uuid,
        width: usize,
        height: usize,
        path: &PathBuf,
    ) -> Result<()>;

    /// Stream is finished
    async fn on_end(&self, pipeline_id: &Uuid) -> Result<()>;

    /// Force update stream
    async fn on_update(&self, pipeline_id: &Uuid) -> Result<()>;

    /// Stats emitted by the pipeline periodically
    async fn on_stats(&self, pipeline_id: &Uuid, stats: StatsType) -> Result<()>;

    /// Get egress configurations for a connection
    async fn get_egress(&self, conn: &ConnectionInfo) -> Result<Vec<EgressType>>;

    /// Get the MoQ origin for publishing streams
    #[cfg(feature = "egress-moq")]
    async fn get_moq_origin(&self) -> Result<hang::moq_lite::OriginProducer>;
}
