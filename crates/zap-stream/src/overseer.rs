use crate::blossom::{BlobDescriptor, Blossom};
use crate::settings::LndSettings;
use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fedimint_tonic_lnd::verrpc::VersionRequest;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use log::{error, info, warn};
use nostr_sdk::prelude::Coordinate;
use nostr_sdk::{Client, Event, EventBuilder, JsonUtil, Keys, Kind, Tag, ToBech32};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;
use uuid::Uuid;
use zap_stream_core::egress::{EgressConfig, EgressSegment};
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_core::overseer::{IngressInfo, IngressStream, IngressStreamType, Overseer};
use zap_stream_core::pipeline::{EgressType, PipelineConfig};
use zap_stream_core::variant::audio::AudioVariant;
use zap_stream_core::variant::mapping::VariantMapping;
use zap_stream_core::variant::video::VideoVariant;
use zap_stream_core::variant::{StreamMapping, VariantStream};
use zap_stream_core::viewer::ViewerTracker;
use zap_stream_db::{IngestEndpoint, UserStream, UserStreamState, ZapStreamDb};

const STREAM_EVENT_KIND: u16 = 30_311;

#[derive(Clone)]
struct StreamViewerState {
    last_published_count: usize,
    last_update_time: DateTime<Utc>,
}

#[derive(Clone)]
struct ActiveStreamInfo {
    started_at: DateTime<Utc>,
    last_segment_time: Option<DateTime<Utc>>,
}

/// zap.stream NIP-53 overseer
#[derive(Clone)]
pub struct ZapStreamOverseer {
    /// Dir where HTTP server serves files from
    out_dir: String,
    /// Database instance for accounts/streams
    db: ZapStreamDb,
    /// LND node connection
    lnd: fedimint_tonic_lnd::Client,
    /// Nostr client for publishing events
    client: Client,
    /// Nostr keys used to sign events
    keys: Keys,
    /// List of blossom servers to upload segments to
    blossom_servers: Vec<Blossom>,
    /// Public facing URL pointing to [out_dir]
    public_url: String,
    /// Currently active streams with timing info
    /// Any streams which are not contained in this map are dead
    active_streams: Arc<RwLock<HashMap<Uuid, ActiveStreamInfo>>>,
    /// Viewer tracking for active streams
    viewer_tracker: ViewerTracker,
    /// Track last published viewer count and update time for each stream
    stream_viewer_states: Arc<RwLock<HashMap<String, StreamViewerState>>>,
}

impl ZapStreamOverseer {
    pub async fn new(
        out_dir: &String,
        public_url: &String,
        private_key: &str,
        db: &str,
        lnd: &LndSettings,
        relays: &Vec<String>,
        blossom_servers: &Option<Vec<String>>,
    ) -> Result<Self> {
        let db = ZapStreamDb::new(db).await?;
        db.migrate().await?;

        #[cfg(debug_assertions)]
        {
            let uid = db.upsert_user(&[0; 32]).await?;
            db.update_user_balance(uid, 100_000_000).await?;
            let user = db.get_user(uid).await?;

            info!(
                "ZERO pubkey: uid={},key={},balance={}",
                user.id,
                user.stream_key,
                user.balance / 1000
            );
        }

        let mut lnd = fedimint_tonic_lnd::connect(
            lnd.address.clone(),
            PathBuf::from(&lnd.cert),
            PathBuf::from(&lnd.macaroon),
        )
        .await?;

        let version = lnd
            .versioner()
            .get_version(VersionRequest::default())
            .await?;
        info!("LND connected: v{}", version.into_inner().version);

        let keys = Keys::from_str(private_key)?;
        let client = nostr_sdk::ClientBuilder::new().signer(keys.clone()).build();
        for r in relays {
            client.add_relay(r).await?;
        }
        client.connect().await;

        let overseer = Self {
            out_dir: out_dir.clone(),
            db,
            lnd,
            client,
            keys,
            blossom_servers: blossom_servers
                .as_ref()
                .unwrap_or(&Vec::new())
                .into_iter()
                .map(|b| Blossom::new(b))
                .collect(),
            public_url: public_url.clone(),
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            viewer_tracker: ViewerTracker::new(),
            stream_viewer_states: Arc::new(RwLock::new(HashMap::new())),
        };

        Ok(overseer)
    }

    pub fn database(&self) -> ZapStreamDb {
        self.db.clone()
    }

    pub fn lnd_client(&self) -> fedimint_tonic_lnd::Client {
        self.lnd.clone()
    }

    pub fn viewer_tracker(&self) -> &ViewerTracker {
        &self.viewer_tracker
    }

    fn stream_to_event_builder(&self, stream: &UserStream) -> Result<EventBuilder> {
        let mut tags = vec![
            Tag::parse(&["d".to_string(), stream.id.to_string()])?,
            Tag::parse(&["status".to_string(), stream.state.to_string()])?,
            Tag::parse(&["starts".to_string(), stream.starts.timestamp().to_string()])?,
        ];
        if let Some(ref ends) = stream.ends {
            tags.push(Tag::parse(&[
                "ends".to_string(),
                ends.timestamp().to_string(),
            ])?);
        }
        if let Some(ref title) = stream.title {
            tags.push(Tag::parse(&["title".to_string(), title.to_string()])?);
        }
        if let Some(ref summary) = stream.summary {
            tags.push(Tag::parse(&["summary".to_string(), summary.to_string()])?);
        }
        if let Some(ref image) = stream.image {
            tags.push(Tag::parse(&["image".to_string(), image.to_string()])?);
        }
        if let Some(ref thumb) = stream.thumb {
            tags.push(Tag::parse(&["thumb".to_string(), thumb.to_string()])?);
        }
        if let Some(ref content_warning) = stream.content_warning {
            tags.push(Tag::parse(&[
                "content_warning".to_string(),
                content_warning.to_string(),
            ])?);
        }
        if let Some(ref goal) = stream.goal {
            tags.push(Tag::parse(&["goal".to_string(), goal.to_string()])?);
        }
        if let Some(ref pinned) = stream.pinned {
            tags.push(Tag::parse(&["pinned".to_string(), pinned.to_string()])?);
        }
        if let Some(ref tags_csv) = stream.tags {
            for tag in tags_csv.split(',') {
                tags.push(Tag::parse(&["t".to_string(), tag.to_string()])?);
            }
        }

        // Add current viewer count for live streams
        if stream.state == UserStreamState::Live {
            let viewer_count = self.viewer_tracker.get_viewer_count(&stream.id);
            tags.push(Tag::parse(&[
                "current_participants".to_string(),
                viewer_count.to_string(),
            ])?);
        }

        let kind = Kind::from(STREAM_EVENT_KIND);
        let coord = Coordinate::new(kind, self.keys.public_key).identifier(&stream.id);
        tags.push(Tag::parse([
            "alt",
            &format!("Watch live on https://zap.stream/{}", coord.to_bech32()?),
        ])?);
        Ok(EventBuilder::new(kind, "").tags(tags))
    }

    fn blob_to_event_builder(&self, stream: &BlobDescriptor) -> Result<EventBuilder> {
        let tags = if let Some(tags) = stream.nip94.as_ref() {
            tags.iter()
                .map_while(|(k, v)| Tag::parse([k, v]).ok())
                .collect()
        } else {
            let mut tags = vec![
                Tag::parse(["x", &stream.sha256])?,
                Tag::parse(["url", &stream.url])?,
                Tag::parse(["size", &stream.size.to_string()])?,
            ];
            if let Some(m) = stream.mime_type.as_ref() {
                tags.push(Tag::parse(["m", m])?)
            }
            tags
        };

        Ok(EventBuilder::new(Kind::FileMetadata, "").tags(tags))
    }

    pub async fn publish_stream_event(
        &self,
        stream: &UserStream,
        pubkey: &Vec<u8>,
    ) -> Result<Event> {
        let extra_tags = vec![
            Tag::parse(["p", hex::encode(pubkey).as_str(), "", "host"])?,
            Tag::parse([
                "streaming",
                self.map_to_stream_public_url(stream, "live.m3u8")?.as_str(),
            ])?,
            Tag::parse([
                "image",
                self.map_to_stream_public_url(stream, "thumb.webp")?
                    .as_str(),
            ])?,
            Tag::parse(["service", self.map_to_public_url("api/v1")?.as_str()])?,
        ];
        let ev = self
            .stream_to_event_builder(stream)?
            .tags(extra_tags)
            .sign_with_keys(&self.keys)?;
        self.client.send_event(ev.clone()).await?;
        Ok(ev)
    }

    fn map_to_stream_public_url(&self, stream: &UserStream, path: &str) -> Result<String> {
        self.map_to_public_url(&format!("{}/{}", stream.id, path))
    }

    fn map_to_public_url(&self, path: &str) -> Result<String> {
        let u: Url = self.public_url.parse()?;
        Ok(u.join(path)?.to_string())
    }
}

#[async_trait]
impl Overseer for ZapStreamOverseer {
    async fn check_streams(&self) -> Result<()> {
        let active_streams = self.db.list_live_streams().await?;
        let now = Utc::now();

        for stream in active_streams {
            // check if stream is alive
            let id = Uuid::parse_str(&stream.id)?;
            info!("Checking stream is alive: {}", stream.id);

            let (is_active, should_timeout) = {
                let streams = self.active_streams.read().await;
                if let Some(stream_info) = streams.get(&id) {
                    // Stream is in active map, but check if it's been inactive too long
                    let timeout = if let Some(last_segment) = stream_info.last_segment_time {
                        // No segments for 60 seconds = timeout
                        (now - last_segment).num_seconds() > 60
                    } else {
                        // No segments yet, but allow 30 seconds for stream to start producing
                        (now - stream_info.started_at).num_seconds() > 30
                    };
                    (true, timeout)
                } else {
                    (false, false)
                }
            };

            if !is_active || should_timeout {
                if should_timeout {
                    warn!("Stream {} timed out - no recent segments", stream.id);
                }
                if let Err(e) = self.on_end(&id).await {
                    error!("Failed to end dead stream {}: {}", &id, e);
                }
            } else {
                // Stream is active, check if we should update viewer count in nostr event
                let viewer_count = self.viewer_tracker.get_viewer_count(&stream.id);
                let now = Utc::now();

                let should_update = {
                    let viewer_states = self.stream_viewer_states.read().await;
                    if let Some(state) = viewer_states.get(&stream.id) {
                        // Update if count changed OR if 10 minutes have passed since last update
                        viewer_count != state.last_published_count
                            || (now - state.last_update_time).num_minutes() >= 10
                    } else {
                        // First time tracking this stream, always update
                        viewer_count > 0
                    }
                };

                if should_update && viewer_count > 0 {
                    if let Ok(user) = self.db.get_user(stream.user_id).await {
                        if let Ok(_) = self.publish_stream_event(&stream, &user.pubkey).await {
                            // Update the tracking state
                            let mut viewer_states = self.stream_viewer_states.write().await;
                            viewer_states.insert(
                                stream.id.clone(),
                                StreamViewerState {
                                    last_published_count: viewer_count,
                                    last_update_time: now,
                                },
                            );
                        } else {
                            warn!("Failed to update viewer count for stream {}", stream.id);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn start_stream(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig> {
        let uid = self
            .db
            .find_user_stream_key(&connection.key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found"))?;

        let user = self.db.get_user(uid).await?;
        if user.balance <= 0 {
            bail!("Not enough balance");
        }

        // Get ingest endpoint configuration based on connection type
        let endpoint = self.detect_endpoint(&connection).await?;

        let cfg = get_variants_from_endpoint(&stream_info, &endpoint)?;

        if cfg.video_src.is_none() || cfg.variants.is_empty() {
            bail!("No video src found");
        }

        let mut egress = vec![];
        egress.push(EgressType::HLS(EgressConfig {
            name: "hls".to_string(),
            variants: cfg.variants.iter().map(|v| v.id()).collect(),
        }));

        let stream_id = Uuid::new_v4();
        // insert new stream record
        let mut new_stream = UserStream {
            id: stream_id.to_string(),
            user_id: uid,
            starts: Utc::now(),
            state: UserStreamState::Live,
            endpoint_id: Some(endpoint.id),
            ..Default::default()
        };
        let stream_event = self.publish_stream_event(&new_stream, &user.pubkey).await?;
        new_stream.event = Some(stream_event.as_json());

        let now = Utc::now();
        let mut streams = self.active_streams.write().await;
        streams.insert(
            stream_id.clone(),
            ActiveStreamInfo {
                started_at: now,
                last_segment_time: None,
            },
        );

        self.db.insert_stream(&new_stream).await?;
        self.db.update_stream(&new_stream).await?;

        Ok(PipelineConfig {
            id: stream_id,
            variants: cfg.variants,
            egress,
            ingress_info: stream_info.clone(),
            video_src: cfg.video_src.unwrap().index,
            audio_src: cfg.audio_src.map(|s| s.index),
        })
    }

    async fn on_segments(
        &self,
        pipeline_id: &Uuid,
        added: &Vec<EgressSegment>,
        deleted: &Vec<EgressSegment>,
    ) -> Result<()> {
        let stream = self.db.get_stream(pipeline_id).await?;

        let duration = added.iter().fold(0.0, |acc, v| acc + v.duration);

        // Get the cost per minute from the ingest endpoint, or use default
        let cost_per_minute = if let Some(endpoint_id) = stream.endpoint_id {
            if let Some(endpoint) = self.db.get_ingest_endpoint(endpoint_id).await? {
                endpoint.cost
            } else {
                0
            }
        } else {
            0
        };

        // Convert duration from seconds to minutes and calculate cost
        let duration_minutes = duration / 60.0;
        let cost = (cost_per_minute as f32 * duration_minutes).round() as i64;
        let bal = self
            .db
            .tick_stream(pipeline_id, stream.user_id, duration, cost)
            .await?;
        if bal <= 0 {
            bail!("Not enough balance");
        }

        // Update last segment time for this stream
        let now = Utc::now();
        {
            let mut streams = self.active_streams.write().await;
            if let Some(info) = streams.get_mut(pipeline_id) {
                info.last_segment_time = Some(now);
            }
        }

        // Upload to blossom servers if configured (N94)
        let mut blobs = vec![];
        for seg in added {
            for b in &self.blossom_servers {
                blobs.push(b.upload(&seg.path, &self.keys, Some("video/mp2t")).await?);
            }
            if let Some(blob) = blobs.first() {
                let a_tag = format!(
                    "{}:{}:{}",
                    STREAM_EVENT_KIND,
                    self.keys.public_key.to_hex(),
                    pipeline_id
                );
                let mut n94 = self.blob_to_event_builder(blob)?.tags([
                    Tag::parse(["a", &a_tag])?,
                    Tag::parse(["d", seg.variant.to_string().as_str()])?,
                    Tag::parse(["index", seg.idx.to_string().as_str()])?,
                ]);

                // some servers add duration tag
                if blob
                    .nip94
                    .as_ref()
                    .map(|a| a.contains_key("duration"))
                    .is_none()
                {
                    n94 = n94.tag(Tag::parse(["duration", seg.duration.to_string().as_str()])?);
                }

                for b in blobs.iter().skip(1) {
                    n94 = n94.tag(Tag::parse(["url", &b.url])?);
                }
                let n94 = n94.sign_with_keys(&self.keys)?;
                let cc = self.client.clone();
                tokio::spawn(async move {
                    if let Err(e) = cc.send_event(n94).await {
                        warn!("Error sending event: {}", e);
                    }
                });
                info!("Published N94 segment to {}", blob.url);
            }
        }

        Ok(())
    }

    async fn on_thumbnail(
        &self,
        pipeline_id: &Uuid,
        width: usize,
        height: usize,
        pixels: &PathBuf,
    ) -> Result<()> {
        // nothing to do
        Ok(())
    }

    async fn on_end(&self, pipeline_id: &Uuid) -> Result<()> {
        let mut stream = self.db.get_stream(pipeline_id).await?;
        let user = self.db.get_user(stream.user_id).await?;

        let mut streams = self.active_streams.write().await;
        streams.remove(pipeline_id);

        // Clean up viewer tracking state for this stream
        let mut viewer_states = self.stream_viewer_states.write().await;
        viewer_states.remove(&stream.id);

        stream.state = UserStreamState::Ended;
        let event = self.publish_stream_event(&stream, &user.pubkey).await?;
        stream.event = Some(event.as_json());
        self.db.update_stream(&stream).await?;

        info!("Stream ended {}", stream.id);
        Ok(())
    }
}

impl ZapStreamOverseer {
    /// Detect which ingest endpoint should be used based on connection info
    async fn detect_endpoint(&self, connection: &ConnectionInfo) -> Result<IngestEndpoint> {
        let endpoints = self.db.get_ingest_endpoints().await?;

        let default = endpoints.iter().max_by_key(|e| e.cost);
        Ok(endpoints
            .iter()
            .find(|e| e.name == connection.endpoint)
            .or(default)
            .unwrap()
            .clone())
    }
}

struct EndpointConfig<'a> {
    video_src: Option<&'a IngressStream>,
    audio_src: Option<&'a IngressStream>,
    variants: Vec<VariantStream>,
}

fn get_variants_from_endpoint<'a>(
    info: &'a IngressInfo,
    endpoint: &zap_stream_db::IngestEndpoint,
) -> Result<EndpointConfig<'a>> {
    let capabilities_str = endpoint.capabilities.as_deref().unwrap_or("");
    let capabilities: Vec<&str> = capabilities_str.split(',').collect();

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
        let parts: Vec<&str> = capability.split(':').collect();

        if parts.len() >= 2 && parts[0] == "variant" && parts[1] == "source" {
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
        } else if parts.len() >= 3 && parts[0] == "variant" {
            if let (Ok(target_height), Ok(bitrate)) =
                (parts[1].parse::<u32>(), parts[2].parse::<u32>())
            {
                // Add video variant for this group
                if let Some(video_src) = video_src {
                    // Calculate dimensions maintaining aspect ratio
                    let input_width = video_src.width as f32;
                    let input_height = video_src.height as f32;
                    let aspect_ratio = input_width / input_height;

                    let output_height = target_height;
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
                    } as u16;

                    vars.push(VariantStream::Video(VideoVariant {
                        mapping: VariantMapping {
                            id: Uuid::new_v4(),
                            src_index: video_src.index,
                            dst_index,
                            group_id,
                        },
                        width: output_width,
                        height: output_height,
                        fps: video_src.fps,
                        bitrate: bitrate as u64,
                        codec: "libx264".to_string(),
                        profile: 77, // AV_PROFILE_H264_MAIN
                        level: 51,
                        keyframe_interval: video_src.fps as u16 * 2,
                        pixel_format: AV_PIX_FMT_YUV420P as u32,
                    }));
                    dst_index += 1;
                }

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
        // Handle other capabilities like dvr:720h here if needed
    }

    Ok(EndpointConfig {
        audio_src,
        video_src,
        variants: vars,
    })
}
