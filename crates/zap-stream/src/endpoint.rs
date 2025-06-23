use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use log::{info, warn};
use uuid::Uuid;
use zap_stream_core::overseer::{IngressInfo, IngressStream, IngressStreamType};
use zap_stream_core::variant::audio::AudioVariant;
use zap_stream_core::variant::mapping::VariantMapping;
use zap_stream_core::variant::video::VideoVariant;
use zap_stream_core::variant::VariantStream;

pub struct EndpointConfig<'a> {
    pub video_src: Option<&'a IngressStream>,
    pub audio_src: Option<&'a IngressStream>,
    pub variants: Vec<VariantStream>,
}

pub enum EndpointCapability {
    SourceVariant,
    Variant { height: u16, bitrate: u64 },
    DVR { height: u16 },
}

pub fn parse_capabilities(cap: &Option<String>) -> Vec<EndpointCapability> {
    if let Some(cap) = cap {
        cap.to_ascii_lowercase()
            .split(',')
            .map_while(|c| {
                let cs = c.split(':').collect::<Vec<&str>>();
                match cs[0] {
                    "variant" if cs[1] == "source" => Some(EndpointCapability::SourceVariant),
                    "variant" if cs.len() == 3 => {
                        if let (Ok(h), Ok(br)) = (cs[1].parse(), cs[2].parse()) {
                            Some(EndpointCapability::Variant {
                                height: h,
                                bitrate: br,
                            })
                        } else {
                            warn!("Invalid variant: {}", c);
                            None
                        }
                    }
                    "dvr" if cs.len() == 2 => {
                        if let Ok(h) = cs[1].parse() {
                            Some(EndpointCapability::DVR { height: h })
                        } else {
                            warn!("Invalid dvr: {}", c);
                            None
                        }
                    }
                    _ => None,
                }
            })
            .collect()
    } else {
        vec![]
    }
}

pub fn get_variants_from_endpoint<'a>(
    info: &'a IngressInfo,
    capabilities: &Vec<EndpointCapability>,
) -> Result<EndpointConfig<'a>> {
    let mut vars: Vec<VariantStream> = vec![];

    let video_src = info
        .streams
        .iter()
        .find(|c| c.stream_type == IngressStreamType::Video);
    let audio_src = info
        .streams
        .iter()
        .find(|c| c.stream_type == IngressStreamType::Audio);

    // Parse all variant capabilities and create grouped variants
    let mut group_id = 0usize;
    let mut dst_index = 0;

    for capability in capabilities {
        match capability {
            EndpointCapability::SourceVariant => {
                // Add copy variant (group for source)
                if let Some(video_src) = video_src {
                    vars.push(VariantStream::CopyVideo(VariantMapping {
                        id: Uuid::new_v4(),
                        src_index: video_src.index,
                        dst_index,
                        group_id,
                    }));
                    dst_index += 1;
                }

                if let Some(audio_src) = audio_src {
                    vars.push(VariantStream::CopyAudio(VariantMapping {
                        id: Uuid::new_v4(),
                        src_index: audio_src.index,
                        dst_index,
                        group_id,
                    }));
                    dst_index += 1;
                }

                group_id += 1;
            }
            EndpointCapability::Variant { height, bitrate } => {
                // Add video variant for this group
                if let Some(video_src) = video_src {
                    let output_height = *height;
                    if video_src.height < output_height as _ {
                        info!(
                            "Skipping variant {}p, source would be upscaled from {}p",
                            height, video_src.height
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

                    vars.push(VariantStream::Video(VideoVariant {
                        mapping: VariantMapping {
                            id: Uuid::new_v4(),
                            src_index: video_src.index,
                            dst_index,
                            group_id,
                        },
                        width: output_width,
                        height: output_height as _,
                        fps: video_src.fps,
                        bitrate: *bitrate as _,
                        codec: "libx264".to_string(),
                        profile: 77, // AV_PROFILE_H264_MAIN
                        level: 51,   // High 5.1 (4K)
                        keyframe_interval: video_src.fps as u16,
                        pixel_format: AV_PIX_FMT_YUV420P as u32,
                    }));
                    dst_index += 1;

                    // Add audio variant for the same group
                    if let Some(audio_src) = audio_src {
                        vars.push(VariantStream::Audio(AudioVariant {
                            mapping: VariantMapping {
                                id: Uuid::new_v4(),
                                src_index: audio_src.index,
                                dst_index,
                                group_id,
                            },
                            bitrate: 192_000,
                            codec: "aac".to_string(),
                            channels: 2,
                            sample_rate: 48_000,
                            sample_fmt: "fltp".to_owned(),
                        }));
                        dst_index += 1;
                    }

                    group_id += 1;
                }
            }
            _ => {}
        }
    }

    Ok(EndpointConfig {
        audio_src,
        video_src,
        variants: vars,
    })
}
