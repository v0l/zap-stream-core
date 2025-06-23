use crate::blossom::{BlobDescriptor, Blossom};
use crate::settings::LndSettings;
use crate::stream_manager::{ActiveStreamInfo, StreamManager, StreamViewerState};
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
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::egress::recorder::RecorderEgress;
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

/// zap.stream NIP-53 overseer
#[derive(Clone)]
pub struct ZapStreamOverseer {
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
    /// Stream manager handles viewer tracking and Nostr publishing
    stream_manager: StreamManager,
}

impl ZapStreamOverseer {
    pub async fn new(
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
            stream_manager: StreamManager::new(),
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
        self.stream_manager.viewer_tracker()
    }

    pub fn stream_manager(&self) -> &StreamManager {
        &self.stream_manager
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
            let viewer_count = self.stream_manager.get_viewer_count(&stream.id);
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
        let pipeline_dir = PathBuf::from(stream.id.to_string());
        let mut extra_tags = vec![
            Tag::parse(["p", hex::encode(pubkey).as_str(), "", "host"])?,
            Tag::parse([
                "image",
                self.map_to_public_url(pipeline_dir.join("thumb.webp").to_str().unwrap())?
                    .as_str(),
            ])?,
            Tag::parse(["service", self.map_to_public_url("api/v1")?.as_str()])?,
        ];
        match stream.state {
            UserStreamState::Live => {
                extra_tags.push(Tag::parse([
                    "streaming",
                    self.map_to_public_url(
                        pipeline_dir
                            .join(HlsEgress::PATH)
                            .join("live.m3u8")
                            .to_str()
                            .unwrap(),
                    )?
                    .as_str(),
                ])?);
            }
            UserStreamState::Ended => {
                if let Some(ep) = stream.endpoint_id {
                    let endpoint = self.db.get_ingest_endpoint(ep).await?;
                    let caps = parse_capabilities(&endpoint.capabilities);
                    let has_recording = caps
                        .iter()
                        .any(|c| matches!(c, EndpointCapability::DVR { .. }));
                    if has_recording {
                        extra_tags.push(Tag::parse([
                            "recording",
                            self.map_to_public_url(
                                pipeline_dir
                                    .join(RecorderEgress::FILENAME)
                                    .to_str()
                                    .unwrap(),
                            )?
                            .as_str(),
                        ])?);
                    }
                }
            }
            _ => {}
        }
        let ev = self
            .stream_to_event_builder(stream)?
            .tags(extra_tags)
            .sign_with_keys(&self.keys)?;
        self.client.send_event(ev.clone()).await?;
        Ok(ev)
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

            let (is_active, should_timeout) = self.stream_manager.check_stream_status(&id).await;

            if !is_active || should_timeout {
                if should_timeout {
                    warn!("Stream {} timed out - no recent segments", stream.id);
                }
                if let Err(e) = self.on_end(&id).await {
                    error!("Failed to end dead stream {}: {}", &id, e);
                }
            } else {
                // Stream is active, check if we should update viewer count in nostr event
                if let Ok(user) = self.db.get_user(stream.user_id).await {
                    let _ = self
                        .stream_manager
                        .check_and_update_viewer_count(&stream, |s| {
                            self.publish_stream_event(s, &user.pubkey)
                        })
                        .await;
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

        let caps = parse_capabilities(&endpoint.capabilities);
        let cfg = get_variants_from_endpoint(&stream_info, &caps)?;

        if cfg.video_src.is_none() || cfg.variants.is_empty() {
            bail!("No video src found");
        }

        let mut egress = vec![];
        egress.push(EgressType::HLS(EgressConfig {
            name: "hls".to_string(),
            variants: cfg.variants.iter().map(|v| v.id()).collect(),
        }));
        if let Some(EndpointCapability::DVR { height }) = caps
            .iter()
            .find(|c| matches!(c, EndpointCapability::DVR { .. }))
        {
            let var = cfg.variants.iter().find(|v| match v {
                VariantStream::Video(v) => v.height == *height,
                _ => false,
            });
            match var {
                Some(var) => {
                    // take all streams in the same group as the matching video resolution (video+audio)
                    let vars_in_group = cfg
                        .variants
                        .iter()
                        .filter(|v| v.group_id() == var.group_id());
                    egress.push(EgressType::Recorder(EgressConfig {
                        name: "dvr".to_string(),
                        variants: vars_in_group.map(|v| v.id()).collect(),
                    }))
                }
                None => {
                    warn!(
                        "Invalid DVR config, no variant found with height {}",
                        height
                    );
                }
            }
        }

        let stream_id = connection.id.clone();
        // insert new stream record
        let mut new_stream = UserStream {
            id: stream_id.to_string(),
            user_id: uid,
            starts: Utc::now(),
            state: UserStreamState::Live,
            endpoint_id: Some(endpoint.id),
            title: user.title.clone(),
            summary: user.summary.clone(),
            thumb: user.image.clone(),
            content_warning: user.content_warning.clone(),
            goal: user.goal.clone(),
            tags: user.tags.clone(),
            ..Default::default()
        };
        let stream_event = self.publish_stream_event(&new_stream, &user.pubkey).await?;
        new_stream.event = Some(stream_event.as_json());

        self.stream_manager
            .add_active_stream(stream_id.clone())
            .await;

        self.db.insert_stream(&new_stream).await?;
        self.db.update_stream(&new_stream).await?;

        Ok(PipelineConfig {
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
            let ep = self.db.get_ingest_endpoint(endpoint_id).await?;
            ep.cost
        } else {
            bail!("Endpoint id not set on stream");
        };

        // Convert duration from seconds to minutes and calculate cost
        let duration_minutes = duration / 60.0;
        let cost = (cost_per_minute as f32 * duration_minutes).round() as i64;
        let bal = self
            .db
            .tick_stream(pipeline_id, stream.user_id, duration, cost)
            .await?;
        if bal <= 0 {
            bail!("Balance has run out");
        }

        // Update last segment time for this stream
        self.stream_manager
            .update_stream_segment_time(pipeline_id)
            .await;

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

        self.stream_manager.remove_active_stream(pipeline_id).await;

        stream.state = UserStreamState::Ended;
        stream.ends = Some(Utc::now());
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
            .find(|e| e.name.eq_ignore_ascii_case(connection.endpoint))
            .or(default)
            .unwrap()
            .clone())
    }
}
