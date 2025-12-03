use crate::egress::{EgressEncoderConfig, EncoderParam, EncoderParams};
use crate::overseer::{IngressInfo, IngressStream, IngressStreamType};
use crate::pipeline::{EgressConfig, EgressType};
use crate::variant::{AudioVariant, VariantGroup, VariantStream, VideoVariant};
use anyhow::{Result, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::av_d2q;
use itertools::Itertools;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use tracing::{info, warn};
use uuid::Uuid;

pub struct EndpointConfigEngine;

impl EndpointConfigEngine {
    /// Generates variant stream configurations from ingress stream information and endpoint capabilities.
    ///
    /// This function analyzes the incoming media streams and the requested endpoint capabilities
    /// to produce a set of output variant streams suitable for transcoding or copying. Each variant
    /// is assigned to a group, where a group typically contains one video stream and one audio stream
    /// that are meant to be muxed together.
    ///
    /// # Arguments
    ///
    /// * `info` - The ingress stream information containing metadata about available source streams
    ///   (video dimensions, codec, fps, audio channels, sample rate, etc.)
    /// * `capabilities` - A list of [`EndpointCapability`] values specifying what output variants
    ///   should be created (e.g., source passthrough, specific resolution/bitrate transcodes)
    ///
    /// # Returns
    ///
    /// Returns an [`EndpointConfig`] containing:
    /// - References to the source video and audio streams (if present)
    /// - A vector of [`VariantStream`] configurations for transcoding/copying
    ///
    /// # Variant Generation Logic
    ///
    /// For each capability in the list:
    ///
    /// - **`SourceVariant`**: Creates a copy (passthrough) variant for video and a transcoded AAC
    ///   audio variant at 192kbps. The video codec is preserved as-is.
    ///
    /// - **`Variant { height, bitrate }`**: Creates a transcoded H.264 video variant at the specified
    ///   height and bitrate, plus an AAC audio variant at 192kbps. The width is calculated to
    ///   maintain the source aspect ratio, and dimensions are adjusted to be even (required by H.264).
    ///   This variant is skipped if:
    ///   - The source resolution is lower than the target (would require upscaling)
    ///   - The source resolution exactly matches the target and a `SourceVariant` is also enabled
    ///
    /// - **`DVR`**: Currently not processed (reserved for future DVR functionality)
    ///
    /// # Group IDs and Destination Indices
    ///
    /// Each variant group gets a unique `group_id` (incremented per capability). Within each group,
    /// video and audio streams receive sequential `dst_index` values, which are used by downstream
    /// muxers to identify output streams.
    ///
    /// # Audio Handling
    ///
    /// Audio is always transcoded to AAC at 192kbps for maximum compatibility. Sample rates are
    /// normalized to either 44.1kHz, 48kHz, or defaulted to 48kHz if the source has a non-standard rate.
    pub fn get_variants_from_endpoint<'a>(
        info: &'a IngressInfo,
        capabilities: &Vec<EndpointCapability>,
        egress: &Vec<EgressType>,
    ) -> Result<EndpointConfig<'a>> {
        let mut variants = Vec::new();
        let mut egress_map = HashMap::new();
        let mut egress = egress.clone();

        // Recorder egress cannot be included in the input egress config
        // and is enabled only by the capabilities
        if egress
            .iter()
            .any(|e| matches!(e, EgressType::Recorder { .. }))
        {
            bail!("Recorder egress cannot be included at this stage.");
        }

        // pick the highest quality ingress stream for transcoding
        let transcode_video_src = info
            .streams
            .iter()
            .filter(|a| a.stream_type == IngressStreamType::Video)
            .max_by_key(|a| a.width * a.height); // TODO: filter by codec
        let transcode_audio_src = info
            .streams
            .iter()
            .filter(|a| a.stream_type == IngressStreamType::Audio)
            .max_by_key(|a| a.sample_rate * a.channels as usize); // TODO: filter by codec (opus vs aac)

        let mut dup_map = HashMap::new();
        for capability in capabilities.iter().sorted() {
            match capability {
                EndpointCapability::SourceVariant => {
                    // for all source streams create the grouped ingress
                    let copy_groups: Vec<_> = info
                        .streams
                        .iter()
                        .filter(|a| a.stream_type == IngressStreamType::Video)
                        .map(|s| {
                            // for each video stream create a mapping to the transcode audio src
                            // usually there is only a single ingress audio stream so we just want
                            // to copy that one for each of the video tracks
                            GroupedIngressStream {
                                video: Some(s),
                                audio: transcode_audio_src,
                                video_params: ingress_stream_to_params(s),
                                audio_params: if let Some(a) = transcode_audio_src {
                                    ingress_stream_to_params(a)
                                } else {
                                    Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .collect();

                    for group in copy_groups {
                        let Some((streams, map)) = Self::get_streams_for_egress(
                            &egress,
                            group.clone(),
                            true,
                            &mut dup_map,
                        ) else {
                            warn!(
                                "No config created for stream group: {:?} with egress {:?}",
                                group, egress
                            );
                            continue;
                        };

                        variants.extend(streams);
                        for (k, v) in map {
                            egress_map.entry(k).or_insert_with(Vec::new).push(v);
                        }
                    }
                }
                EndpointCapability::Variant { height, bitrate } => {
                    // Add video variant for this group
                    if let Some(video_src) = transcode_video_src {
                        let output_height = *height;
                        if video_src.height < output_height as _ {
                            info!(
                                "Skipping variant {}p, source would be upscaled from {}p",
                                height, video_src.height
                            );
                            continue;
                        }

                        // Skip variant if resolution matches an existing variant
                        if variants.iter().any(|v| match v {
                            VariantStream::Video(v) | VariantStream::CopyVideo(v) => {
                                v.height == output_height
                            }
                            _ => false,
                        }) {
                            info!(
                                "Skipping variant {}p, resolution matches an existing video variant.",
                                height
                            );
                            continue;
                        }

                        // Calculate dimensions maintaining aspect ratio
                        let input_width = video_src.width as f32;
                        let input_height = video_src.height as f32;
                        let aspect_ratio = input_width / input_height;

                        let output_width = (output_height as f32 * aspect_ratio).round() as u16;

                        // Ensure even dimensions for H.264 compatibility
                        let output_width = if output_width % 2 == 1 {
                            output_width + 1
                        } else {
                            output_width
                        };
                        let output_height = if output_height % 2 == 1 {
                            output_height + 1
                        } else {
                            output_height
                        };

                        let stream_group = GroupedIngressStream {
                            video: Some(video_src),
                            audio: transcode_audio_src,
                            video_params: ingress_stream_to_params(video_src).with_params(vec![
                                EncoderParam::Bitrate { value: *bitrate },
                                EncoderParam::Height {
                                    value: output_height,
                                },
                                EncoderParam::Width {
                                    value: output_width,
                                },
                            ]),
                            audio_params: if let Some(a) = transcode_audio_src {
                                ingress_stream_to_params(a)
                            } else {
                                Default::default()
                            },
                            ..Default::default()
                        };

                        let Some((streams, map)) = Self::get_streams_for_egress(
                            &egress,
                            stream_group.clone(),
                            false,
                            &mut dup_map,
                        ) else {
                            warn!(
                                "No config created for stream group: {:?} with egress {:?}",
                                stream_group, egress
                            );
                            continue;
                        };
                        variants.extend(streams);
                        for (k, v) in map {
                            egress_map.entry(k).or_insert_with(Vec::new).push(v);
                        }
                    }
                }
                EndpointCapability::DVR { height } => {
                    if let Some(var) = variants.iter().find(|v| match v {
                        VariantStream::Video(v) | VariantStream::CopyVideo(v) => {
                            v.height == *height
                        }
                        _ => false,
                    }) {
                        // insert the Recorder egress
                        let id = Uuid::new_v4();
                        egress.push(EgressType::Recorder {
                            id,
                            height: *height,
                        });
                        let e_map = egress_map.entry(id).or_insert(Vec::new());
                        e_map.push(VariantGroup {
                            video: Some(var.id()),
                            audio: transcode_audio_src.and_then(|s| {
                                variants
                                    .iter()
                                    .find(|v| v.src_index() == s.index)
                                    .map(|s| s.id())
                            }),
                            ..Default::default()
                        });
                    } else {
                        warn!("Could not configure DVR capability, no variant {}p", height);
                    }
                }
            }
        }

        Ok(EndpointConfig {
            audio_src: transcode_audio_src,
            video_src: transcode_video_src,
            variants,
            egress_map,
            egress: egress.clone(),
        })
    }

    fn get_streams_for_egress(
        egress: &Vec<EgressType>,
        stream: GroupedIngressStream,
        copy: bool,
        dup_map: &mut HashMap<EgressEncoderConfig, Uuid>,
    ) -> Option<(Vec<VariantStream>, HashMap<Uuid, VariantGroup>)> {
        // ask each egress what codec params it wants, this creates a grouped stream per egress
        // once all groups are created we deduplicate them using the PartialEq impl
        // This should mean that 2 egress' with the same requests should result in a single
        // grouped stream which both egress' can use, so we don't transcode it multiple times
        let streams: Vec<_> = egress
            .iter()
            .flat_map(|e| {
                // create a mapping for (Egress, EncoderParams, src_index, VariantId)
                let mut params = vec![];
                if let Some(v) = stream.video
                    && let Some(p) = e.get_encoder_params(v, &stream.video_params)
                {
                    // if we're supposed to copy this stream make sure the codec is the same
                    // if the egress expects a different codec we cant copy it
                    // TODO: check params?
                    if !copy || (copy && p.codec == v.codec_name().unwrap_or("".to_string())) {
                        params.push((e, p, v.index, Uuid::new_v4()));
                    } else {
                        warn!(
                            "Failed to get encoder params, could not copy #{} to {:?}",
                            v.index, e
                        )
                    }
                }
                if let Some(a) = stream.audio
                    && let Some(p) = e.get_encoder_params(a, &stream.audio_params)
                {
                    if !copy || (copy && p.codec == a.codec_name().unwrap_or("".to_string())) {
                        params.push((e, p, a.index, Uuid::new_v4()));
                    } else {
                        warn!(
                            "Failed to get encoder params, could not copy #{} to {:?}",
                            a.index, e
                        )
                    }
                }
                if let Some(s) = stream.subtitle
                    && let Some(p) = e.get_encoder_params(s, &stream.subtitle_params)
                {
                    if !copy || (copy && p.codec == s.codec_name().unwrap_or("".to_string())) {
                        params.push((e, p, s.index, Uuid::new_v4()));
                    } else {
                        warn!(
                            "Failed to get encoder params, could not copy #{} to {:?}",
                            s.index, e
                        )
                    }
                }
                params
            })
            .collect();

        if streams.is_empty() {
            return None;
        }

        let mut egress_map: HashMap<Uuid, VariantGroup> = HashMap::new();
        let mut ret = Vec::new();
        for (_, chunk) in &streams.into_iter().chunk_by(|c| c.0) {
            for (e, param, src_index, id) in chunk.into_iter() {
                let e_map = egress_map.entry(e.id()).or_insert(Default::default());

                // create a new variant stream using this config and store mapping for deduplication
                match dup_map.entry(param.clone()) {
                    Entry::Occupied(v) => match &param.stream_type {
                        IngressStreamType::Video => {
                            e_map.video.replace(v.get().clone());
                        }
                        IngressStreamType::Audio => {
                            e_map.audio.replace(v.get().clone());
                        }
                        IngressStreamType::Subtitle => {
                            e_map.subtitle.replace(v.get().clone());
                        }
                    },
                    Entry::Vacant(dup) => match &param.stream_type {
                        IngressStreamType::Video => {
                            let mut cfg = VideoVariant {
                                id,
                                src_index,
                                codec: param.codec.clone(),
                                ..Default::default()
                            };
                            cfg.apply_params(&param.codec_params);
                            if copy {
                                ret.push(VariantStream::CopyVideo(cfg));
                            } else {
                                ret.push(VariantStream::Video(cfg));
                            }
                            dup.insert(id);
                            e_map.video.replace(id);
                        }
                        IngressStreamType::Audio => {
                            let mut cfg = AudioVariant {
                                id,
                                src_index,
                                codec: param.codec.clone(),
                                ..Default::default()
                            };
                            cfg.apply_params(&param.codec_params);
                            // Never copy audio streams always transcode
                            ret.push(VariantStream::Audio(cfg));
                            dup.insert(id);
                            e_map.audio.replace(id);
                        }
                        IngressStreamType::Subtitle => {
                            todo!()
                        }
                    },
                }
            }
        }
        Some((ret, egress_map))
    }
}

#[derive(Clone, Debug, Default)]
struct GroupedIngressStream<'a> {
    video: Option<&'a IngressStream>,
    audio: Option<&'a IngressStream>,
    subtitle: Option<&'a IngressStream>,

    video_params: EncoderParams,
    audio_params: EncoderParams,
    subtitle_params: EncoderParams,
}

impl Display for GroupedIngressStream<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut sep = false;
        if let Some(video) = self.video {
            write!(f, "v:{}", video.index)?;
            sep = true;
        }
        if sep {
            write!(f, ", ")?;
        }
        if let Some(audio) = self.audio {
            write!(f, "a:{}", audio.index)?;
            sep = true;
        }
        if sep {
            write!(f, ", ")?;
        }
        if let Some(subtitle) = self.subtitle {
            write!(f, "s:{}", subtitle.index)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct EndpointConfig<'a> {
    pub video_src: Option<&'a IngressStream>,
    pub audio_src: Option<&'a IngressStream>,
    /// Distinct variant streams for the transcode pipeline
    pub variants: Vec<VariantStream>,
    /// Mapping egress types to variant stream ids
    pub egress_map: HashMap<Uuid, Vec<VariantGroup>>,
    /// The configured egress'
    pub egress: Vec<EgressType>,
}

impl EndpointConfig<'_> {
    pub fn get_egress_configs(&self) -> Vec<EgressConfig> {
        self.egress_map
            .iter()
            .filter_map(|e| {
                let kind = self.egress.iter().find(|c| c.id() == *e.0)?;
                Some(EgressConfig {
                    kind: kind.clone(),
                    variants: e.1.clone(),
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EndpointCapability {
    SourceVariant,
    Variant { height: u16, bitrate: u64 },
    DVR { height: u16 },
}

impl EndpointCapability {
    /// Returns the sort order for capability types: SourceVariant=0, Variant=1, DVR=2
    fn type_order(&self) -> u8 {
        match self {
            EndpointCapability::SourceVariant => 0,
            EndpointCapability::Variant { .. } => 1,
            EndpointCapability::DVR { .. } => 2,
        }
    }
}

impl Ord for EndpointCapability {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        // First compare by type order
        match self.type_order().cmp(&other.type_order()) {
            Ordering::Equal => {}
            ord => return ord,
        }

        // If same type, compare by height and bitrate (descending for variants)
        match (self, other) {
            (
                EndpointCapability::Variant {
                    height: h1,
                    bitrate: b1,
                },
                EndpointCapability::Variant {
                    height: h2,
                    bitrate: b2,
                },
            ) => {
                // Sort by height descending, then bitrate descending
                match h2.cmp(h1) {
                    Ordering::Equal => b2.cmp(b1),
                    ord => ord,
                }
            }
            (EndpointCapability::DVR { height: h1 }, EndpointCapability::DVR { height: h2 }) => {
                // Sort DVR by height descending
                h2.cmp(h1)
            }
            _ => Ordering::Equal,
        }
    }
}

impl PartialOrd for EndpointCapability {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for EndpointCapability {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EndpointCapability::SourceVariant => write!(f, "variant:source"),
            EndpointCapability::Variant { height, bitrate } => {
                write!(f, "variant:{}:{}", height, bitrate)
            }
            EndpointCapability::DVR { height } => write!(f, "dvr:{}", height),
        }
    }
}

impl FromStr for EndpointCapability {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let cs = s.split(':').collect::<Vec<&str>>();
        match cs[0] {
            "variant" if cs[1] == "source" => Ok(EndpointCapability::SourceVariant),
            "variant" if cs.len() == 3 => {
                if let (Ok(h), Ok(br)) = (cs[1].parse(), cs[2].parse()) {
                    Ok(EndpointCapability::Variant {
                        height: h,
                        bitrate: br,
                    })
                } else {
                    bail!("Invalid variant: {}", s);
                }
            }
            "dvr" if cs.len() == 2 => {
                if let Ok(h) = cs[1].parse() {
                    Ok(EndpointCapability::DVR { height: h })
                } else {
                    bail!("Invalid dvr: {}", s);
                }
            }
            _ => bail!("Invalid dvr: {}", s),
        }
    }
}

impl Display for EndpointConfig<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "EndpointConfig:")?;
        writeln!(f, "├── Variants ({}):", self.variants.len())?;
        for (i, variant) in self.variants.iter().enumerate() {
            let prefix = if i == self.variants.len() - 1 {
                "│   └──"
            } else {
                "│   ├──"
            };
            writeln!(f, "{} {}", prefix, variant)?;
        }

        writeln!(f, "└── Egress Mappings:")?;
        let egress_count = self.egress.len();
        for (i, egress) in self.egress.iter().enumerate() {
            let egress_id = egress.id();
            let is_last_egress = i == egress_count - 1;
            let egress_prefix = if is_last_egress {
                "    └──"
            } else {
                "    ├──"
            };
            let child_prefix = if is_last_egress {
                "       "
            } else {
                "    │  "
            };

            // Get egress type name
            let egress_name = match egress {
                EgressType::HLS { .. } => "HLS",
                EgressType::Recorder { .. } => "Recorder",
                EgressType::RTMPForwarder { .. } => "RTMPForwarder",
                EgressType::Moq { .. } => "MoQ",
            };

            writeln!(f, "{} {} ({})", egress_prefix, egress_name, egress_id)?;

            // Get groups for this egress
            if let Some(groups) = self.egress_map.get(&egress_id) {
                let group_count = groups.len();
                for (j, group) in groups.iter().enumerate() {
                    let is_last_group = j == group_count - 1;
                    let group_prefix = if is_last_group {
                        "└──"
                    } else {
                        "├──"
                    };
                    let variant_prefix = if is_last_group { "   " } else { "│  " };

                    writeln!(f, "{} {} Group ({})", child_prefix, group_prefix, group.id)?;

                    // Show variants in this group
                    let mut variant_entries = Vec::new();
                    if let Some(video_id) = &group.video {
                        if let Some(v) = self.variants.iter().find(|v| &v.id() == video_id) {
                            variant_entries.push(format!("{}", v));
                        } else {
                            variant_entries.push(format!("Video: {}", video_id));
                        }
                    }
                    if let Some(audio_id) = &group.audio {
                        if let Some(a) = self.variants.iter().find(|v| &v.id() == audio_id) {
                            variant_entries.push(format!("{}", a));
                        } else {
                            variant_entries.push(format!("Audio: {}", audio_id));
                        }
                    }
                    if let Some(sub_id) = &group.subtitle {
                        variant_entries.push(format!("Subtitle: {}", sub_id));
                    }

                    let entry_count = variant_entries.len();
                    for (k, entry) in variant_entries.iter().enumerate() {
                        let is_last_entry = k == entry_count - 1;
                        let entry_prefix = if is_last_entry {
                            "└──"
                        } else {
                            "├──"
                        };
                        writeln!(
                            f,
                            "{} {} {} {}",
                            child_prefix, variant_prefix, entry_prefix, entry
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl<'a> EndpointConfig<'a> {
    /// Verify that all variant IDs referenced in egress_map exist in the variants list.
    /// Returns Ok(()) if valid, or Err with a list of missing variant IDs.
    pub fn verify(self) -> Result<Self, HashSet<Uuid>> {
        let variant_ids: HashSet<_> = self.variants.iter().map(|v| v.id()).collect();
        let mut missing = HashSet::new();

        for groups in self.egress_map.values() {
            for group in groups {
                if let Some(video_id) = &group.video {
                    if !variant_ids.contains(video_id) {
                        missing.insert(*video_id);
                    }
                }
                if let Some(audio_id) = &group.audio {
                    if !variant_ids.contains(audio_id) {
                        missing.insert(*audio_id);
                    }
                }
                if let Some(subtitle_id) = &group.subtitle {
                    if !variant_ids.contains(subtitle_id) {
                        missing.insert(*subtitle_id);
                    }
                }
            }
        }

        if missing.is_empty() {
            Ok(self)
        } else {
            Err(missing)
        }
    }
}

pub fn parse_capabilities(cap: &Option<String>) -> Vec<EndpointCapability> {
    if let Some(cap) = cap {
        cap.to_ascii_lowercase()
            .split(',')
            .filter_map(|c| c.parse().ok())
            .collect()
    } else {
        vec![]
    }
}

fn ingress_stream_to_params(stream: &IngressStream) -> EncoderParams {
    let mut ret = vec![];

    match stream.stream_type {
        IngressStreamType::Video => {
            if stream.fps.is_normal() && stream.fps > 0.0 {
                let fps_q = unsafe { av_d2q(stream.fps as _, 90_000) };
                ret.push(EncoderParam::Framerate {
                    num: fps_q.num as _,
                    den: fps_q.den as _,
                });
            }
            if stream.width != 0 {
                ret.push(EncoderParam::Width {
                    value: stream.width as _,
                });
            }
            if stream.height != 0 {
                ret.push(EncoderParam::Height {
                    value: stream.height as _,
                });
            }
            if stream.bitrate != 0 {
                ret.push(EncoderParam::Bitrate {
                    value: stream.bitrate as _,
                });
            }
            if let Ok(name) = stream.pixel_format_name() {
                ret.push(EncoderParam::PixelFormat { name })
            }
        }
        IngressStreamType::Audio => {
            if stream.channels != 0 {
                ret.push(EncoderParam::AudioChannels {
                    count: stream.channels as _,
                });
            }
            if stream.sample_rate != 0 {
                ret.push(EncoderParam::SampleRate {
                    size: stream.sample_rate as _,
                });
            }
            if stream.bitrate != 0 {
                ret.push(EncoderParam::Bitrate {
                    value: stream.bitrate as _,
                });
            }
            if let Ok(name) = stream.sample_format_name() {
                ret.push(EncoderParam::SampleFormat { name })
            }
        }
        IngressStreamType::Subtitle => {
            todo!()
        }
    }
    ret.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::SegmentType;
    use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID;
    use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
    use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_FLTP;

    #[test]
    fn test_endpoint_config() -> Result<()> {
        tracing_subscriber::fmt::try_init().ok();

        let mock_ingress = IngressInfo {
            bitrate: 0,
            streams: vec![
                IngressStream {
                    index: 0,
                    stream_type: IngressStreamType::Video,
                    codec: AVCodecID::AV_CODEC_ID_H264 as _,
                    format: AV_PIX_FMT_YUV420P as _,
                    width: 1920,
                    height: 1080,
                    bitrate: 8_000_000,
                    fps: 30.0,
                    sample_rate: 0,
                    channels: 0,
                    language: "".to_string(),
                },
                IngressStream {
                    index: 1,
                    stream_type: IngressStreamType::Video,
                    codec: AVCodecID::AV_CODEC_ID_H264 as _,
                    format: AV_PIX_FMT_YUV420P as _,
                    width: 1280,
                    height: 720,
                    bitrate: 6_000_000,
                    fps: 30.0,
                    sample_rate: 0,
                    channels: 0,
                    language: "".to_string(),
                },
                IngressStream {
                    index: 2,
                    stream_type: IngressStreamType::Audio,
                    codec: AVCodecID::AV_CODEC_ID_AAC as _,
                    format: AV_SAMPLE_FMT_FLTP as _,
                    width: 0,
                    height: 0,
                    bitrate: 320_000,
                    fps: 0.0,
                    sample_rate: 44_100,
                    channels: 2,
                    language: "en".to_string(),
                },
            ],
        };

        let hls_id = Uuid::new_v4();
        let mock_egress = vec![EgressType::HLS {
            id: hls_id,
            segment_type: SegmentType::FMP4,
            segment_length: 2.0,
        }];

        let mock_caps = vec![
            EndpointCapability::SourceVariant,
            EndpointCapability::Variant {
                height: 1080,
                bitrate: 4_000_000,
            },
            EndpointCapability::Variant {
                height: 720,
                bitrate: 1_000_000,
            },
            EndpointCapability::Variant {
                height: 480,
                bitrate: 500_000,
            },
            EndpointCapability::Variant {
                height: 240,
                bitrate: 200_000,
            },
            EndpointCapability::DVR { height: 720 },
        ];

        let cfg = EndpointConfigEngine::get_variants_from_endpoint(
            &mock_ingress,
            &mock_caps,
            &mock_egress,
        )?;
        println!("{}", cfg);

        // Verify source streams are correctly identified
        assert!(cfg.video_src.is_some());
        assert_eq!(cfg.video_src.unwrap().index, 0); // Highest resolution video (1920x1080)
        assert!(cfg.audio_src.is_some());
        assert_eq!(cfg.audio_src.unwrap().index, 2);

        // Count video and audio variants
        let video_variants: Vec<_> = cfg
            .variants
            .iter()
            .filter(|v| matches!(v, VariantStream::Video(_) | VariantStream::CopyVideo(_)))
            .collect();
        let audio_variants: Vec<_> = cfg
            .variants
            .iter()
            .filter(|v| matches!(v, VariantStream::Audio(_) | VariantStream::CopyAudio(_)))
            .collect();

        // Should have:
        // - 2 CopyVideo (1080p from stream 0, 720p from stream 1) from SourceVariant
        // - 2 Video (480p, 240p) from Variant capabilities (1080p and 720p skipped as they match source)
        // - Audio variants for each group
        assert_eq!(video_variants.len(), 4, "Expected 4 video variants");

        // Verify copy variants exist for both source video streams
        let copy_videos: Vec<_> = cfg
            .variants
            .iter()
            .filter(|v| matches!(v, VariantStream::CopyVideo(_)))
            .collect();
        assert_eq!(copy_videos.len(), 2, "Expected 2 copy video variants");

        // Verify transcoded variants at 480p and 240p
        let transcoded_videos: Vec<_> = cfg
            .variants
            .iter()
            .filter_map(|v| match v {
                VariantStream::Video(v) => Some(v),
                _ => None,
            })
            .collect();
        assert_eq!(
            transcoded_videos.len(),
            2,
            "Expected 2 transcoded video variants"
        );

        let heights: Vec<_> = transcoded_videos.iter().map(|v| v.height).collect();
        assert!(heights.contains(&480), "Expected 480p variant");
        assert!(heights.contains(&240), "Expected 240p variant");

        // Verify egress are copied to config
        assert_eq!(cfg.egress.len(), 2);
        assert_eq!(cfg.egress[0].id(), hls_id);

        // Verify egress mapping exists for each egress
        assert_eq!(cfg.egress_map.len(), 2);

        // Verify each egress has variant groups
        for egress in &cfg.egress {
            let groups = cfg
                .egress_map
                .get(&egress.id())
                .expect("Egress should have groups");
            assert!(
                !groups.is_empty(),
                "Egress should have at least one variant group"
            );

            for group in groups {
                assert!(group.video.is_some(), "Group should have video variant");
                assert!(group.audio.is_some(), "Group should have audio variant");

                // Verify audio is always transcoded (no CopyAudio)
                if let Some(audio_id) = group.audio {
                    let audio_variant = cfg
                        .variants
                        .iter()
                        .find(|v| v.id() == audio_id)
                        .expect("Audio variant should exist");
                    assert!(
                        matches!(audio_variant, VariantStream::Audio(_)),
                        "Egress should only use transcoded audio, not CopyAudio"
                    );
                }
            }
        }

        Ok(())
    }

    #[test]
    fn test_endpoint_capability_sorting() {
        let mut caps = vec![
            EndpointCapability::DVR { height: 720 },
            EndpointCapability::Variant {
                height: 480,
                bitrate: 500_000,
            },
            EndpointCapability::SourceVariant,
            EndpointCapability::DVR { height: 1080 },
            EndpointCapability::Variant {
                height: 1080,
                bitrate: 4_000_000,
            },
            EndpointCapability::Variant {
                height: 720,
                bitrate: 2_000_000,
            },
            EndpointCapability::Variant {
                height: 720,
                bitrate: 1_000_000,
            },
        ];

        caps.sort();

        // Expected order: SourceVariant first, then Variants by height desc/bitrate desc, then DVR by height desc
        assert_eq!(caps[0], EndpointCapability::SourceVariant);
        assert_eq!(
            caps[1],
            EndpointCapability::Variant {
                height: 1080,
                bitrate: 4_000_000
            }
        );
        assert_eq!(
            caps[2],
            EndpointCapability::Variant {
                height: 720,
                bitrate: 2_000_000
            }
        );
        assert_eq!(
            caps[3],
            EndpointCapability::Variant {
                height: 720,
                bitrate: 1_000_000
            }
        );
        assert_eq!(
            caps[4],
            EndpointCapability::Variant {
                height: 480,
                bitrate: 500_000
            }
        );
        assert_eq!(caps[5], EndpointCapability::DVR { height: 1080 });
        assert_eq!(caps[6], EndpointCapability::DVR { height: 720 });
    }
}
