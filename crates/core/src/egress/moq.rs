use crate::egress::{Egress, EgressResult, EncoderVariantGroup};
use crate::variant::VariantStream;
use anyhow::{Result, bail};
use bytes::Bytes;
use ffmpeg_rs_raw::AvPacketRef;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AV_PKT_FLAG_KEY, av_q2d};
use hang::catalog::{AAC, Audio, AudioCodec, H264, H265, VP9, Video, VideoCodec};
use hang::moq_lite::{Broadcast, OriginProducer};
use hang::{Catalog, Frame, Timestamp, TrackProducer, catalog};
use std::collections::HashMap;
use std::slice;
use tokio::runtime::Handle;
use tracing::warn;
use uuid::Uuid;

pub struct MoqEgress {
    handle: Handle,
    /// Track producer handle for stream data
    tracks: HashMap<String, TrackProducer>,
    pts_offset: f64,
}

impl MoqEgress {
    pub const PATH: &'static str = "moq";

    pub fn new<'a>(
        handle: Handle,
        origin: OriginProducer,
        id: &str,
        groups: &Vec<EncoderVariantGroup>,
    ) -> Result<Self> {
        // create a Catalog which contains all the video / audio tracks
        let mut catalog = Catalog::default();
        let mut video_tracks = HashMap::new();
        let mut audio_tracks = HashMap::new();
        let mut track_handles = Vec::new();
        let mut video_priority = 100;
        let mut audio_priority = 1;
        for group in groups {
            for stream in &group.streams {
                match stream.variant {
                    VariantStream::Video(var) | VariantStream::CopyVideo(var) => {
                        let cfg = catalog::VideoConfig {
                            codec: match var.codec.as_str() {
                                "h264" => H264 {
                                    //TODO: take from encoder
                                    profile: 0,
                                    constraints: 0,
                                    level: 0,
                                }
                                .into(),
                                "h265" | "hevc" => H265 {
                                    //TODO: take from encoder
                                    in_band: false,
                                    profile_space: 0,
                                    profile_idc: 0,
                                    profile_compatibility_flags: [0, 0, 0, 0],
                                    tier_flag: false,
                                    level_idc: 0,
                                    constraint_flags: [0, 0, 0, 0, 0, 0],
                                }
                                .into(),
                                "vp8" => VideoCodec::VP8,
                                "vp9" => VP9 {
                                    //TODO: take from encoder
                                    profile: 0,
                                    level: 0,
                                    bit_depth: 0,
                                    chroma_subsampling: 0,
                                    color_primaries: 0,
                                    transfer_characteristics: 0,
                                    matrix_coefficients: 0,
                                    full_range: false,
                                }
                                .into(),
                                _ => bail!("Unsupported video codec {}", &var.codec),
                            },
                            description: None,
                            coded_width: Some(var.width as _),
                            coded_height: Some(var.height as _),
                            display_ratio_width: None,
                            display_ratio_height: None,
                            bitrate: Some(var.bitrate as _),
                            framerate: Some(var.fps as _),
                            optimize_for_latency: Some(true),
                        };
                        video_tracks.insert(var.id.to_string(), cfg);
                        track_handles.push(hang::moq_lite::Track {
                            name: var.id.to_string(),
                            priority: video_priority,
                        });
                        video_priority += 1;
                    }
                    VariantStream::Audio(var) | VariantStream::CopyAudio(var) => {
                        let cfg = catalog::AudioConfig {
                            codec: match var.codec.as_str() {
                                "aac" | "libfdk_aac" => AAC { profile: 0 }.into(),
                                "opus" => AudioCodec::Opus,
                                _ => bail!("Unsupported audio codec {}", &var.codec),
                            },
                            sample_rate: 0,
                            channel_count: 0,
                            bitrate: Some(var.bitrate as _),
                            description: None,
                        };
                        audio_tracks.insert(var.id.to_string(), cfg);
                        track_handles.push(hang::moq_lite::Track {
                            name: var.id.to_string(),
                            priority: audio_priority,
                        });
                        audio_priority += 1;
                    }
                    _ => {}
                }
            }
        }

        if video_tracks.is_empty() && audio_tracks.is_empty() {
            bail!("Must have at least 1 video or audio track")
        }
        if !video_tracks.is_empty() {
            catalog.video = Some(Video {
                renditions: video_tracks,
                priority: 1,
                display: None,
                rotation: None,
                flip: None,
            })
        }
        if !audio_tracks.is_empty() {
            catalog.audio = Some(Audio {
                renditions: audio_tracks,
                priority: 0,
            })
        }
        let mut broadcast = Broadcast::produce();

        let catalog = catalog.produce();
        broadcast.producer.insert_track(catalog.consumer.track);

        // create the tracks
        let mut tracks = HashMap::new();
        for track in track_handles {
            let id = track.name.clone();
            tracks.insert(id, broadcast.producer.create_track(track).into());
        }

        let g = handle.enter();
        if !origin.publish_broadcast(id, broadcast.consumer) {
            bail!("Failed to publish, not allowed")
        }
        drop(g);

        Ok(Self {
            tracks,
            handle,
            pts_offset: 0.0,
        })
    }
}
impl Egress for MoqEgress {
    fn process_pkt(&mut self, packet: AvPacketRef, variant: &Uuid) -> Result<EgressResult> {
        if let Some(track) = self.tracks.get_mut(variant.to_string().as_str()) {
            let is_keyframe = packet.flags & AV_PKT_FLAG_KEY != 0;
            let data_slice = unsafe { slice::from_raw_parts(packet.data, packet.size as _) };
            let mut pts_secs = packet.pts as f64 * unsafe { av_q2d(packet.time_base) };
            pts_secs += self.pts_offset;
            if pts_secs < 0.0 {
                self.pts_offset += pts_secs.abs();
                warn!("PTS was negative, new offset {:.4}s", self.pts_offset);
                pts_secs += self.pts_offset;
            }
            track.write(Frame {
                timestamp: Timestamp::from_secs_f64(pts_secs),
                keyframe: is_keyframe,
                payload: Bytes::copy_from_slice(data_slice),
            })
        }

        Ok(EgressResult::None)
    }

    fn reset(&mut self) -> Result<EgressResult> {
        // TODO: unpublish
        Ok(EgressResult::None)
    }
}
