use crate::settings::{LndSettings, Settings, LowBalanceNotificationConfig};
use crate::stream_manager::StreamManager;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
#[cfg(feature = "zap-stream")]
use fedimint_tonic_lnd::verrpc::VersionRequest;
use log::{error, info, warn};
use nostr_sdk::prelude::Coordinate;
use nostr_sdk::{Client, Event, EventBuilder, JsonUtil, Keys, Kind, NostrSigner, Tag, ToBech32, PublicKey};
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::egress::recorder::RecorderEgress;
use zap_stream_core::egress::EgressSegment;
use zap_stream_core::endpoint::{
    get_variants_from_endpoint, parse_capabilities, EndpointCapability,
};
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_core::mux::SegmentType;
use zap_stream_core::overseer::{IngressInfo, Overseer, StatsType};
use zap_stream_core::pipeline::{EgressType, PipelineConfig};
use zap_stream_core::variant::{StreamMapping, VariantStream};
use zap_stream_core_nostr::n94::{N94Publisher, N94Segment, N94StreamInfo, N94Variant};
use zap_stream_db::{IngestEndpoint, UserStream, UserStreamState, ZapStreamDb};

/// zap.stream NIP-53 overseer
#[derive(Clone)]
pub struct ZapStreamOverseer {
    /// Database instance for accounts/streams
    db: ZapStreamDb,
    #[cfg(feature = "zap-stream")]
    /// LND node connection
    lnd: fedimint_tonic_lnd::Client,
    /// Nostr client for publishing events
    client: Client,
    /// Public facing URL pointing to [out_dir]
    public_url: String,
    /// Stream manager handles viewer tracking
    stream_manager: StreamManager,
    /// NIP-5E publisher
    n94: Option<N94Publisher>,
    /// HLS segment length
    segment_length: f32,
    /// Low balance notification config
    low_balance_config: Option<LowBalanceNotificationConfig>,
    /// Track which streams have been notified about low balance
    notified_streams: Arc<Mutex<HashSet<Uuid>>>,
}

impl ZapStreamOverseer {
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Ok(ZapStreamOverseer::new(
            &settings.public_url,
            &settings.overseer.nsec,
            &settings.overseer.database,
            #[cfg(feature = "zap-stream")]
            &settings.overseer.lnd,
            &settings.overseer.relays,
            &settings.overseer.blossom,
            settings.overseer.segment_length.unwrap_or(2.0),
            settings.overseer.low_balance_notification.clone(),
        )
        .await?)
    }

    pub async fn new(
        public_url: &String,
        private_key: &str,
        db: &str,
        #[cfg(feature = "zap-stream")] lnd: &LndSettings,
        relays: &Vec<String>,
        blossom_servers: &Option<Vec<String>>,
        segment_length: f32,
        low_balance_config: Option<LowBalanceNotificationConfig>,
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

        #[cfg(feature = "zap-stream")]
        let lnd = {
            let mut lnd = fedimint_tonic_lnd::connect(
                lnd.address.clone(),
                PathBuf::from(&lnd.cert),
                PathBuf::from(&lnd.macaroon),
            )
            .await
            .context("Failed to connect to LND")?;

            let version = lnd
                .versioner()
                .get_version(VersionRequest::default())
                .await
                .context("Failed to get LND version")?;
            info!("LND connected: v{}", version.into_inner().version);

            lnd
        };

        let keys = Keys::from_str(private_key)?;
        let client = nostr_sdk::ClientBuilder::new().signer(keys.clone()).build();
        for r in relays {
            client.add_relay(r).await?;
        }
        client.connect().await;

        let overseer = Self {
            db,
            #[cfg(feature = "zap-stream")]
            lnd,
            n94: if let Some(s) = blossom_servers {
                Some(N94Publisher::new(client.clone(), s, 3, segment_length))
            } else {
                None
            },
            client,
            segment_length,
            public_url: public_url.clone(),
            stream_manager: StreamManager::new(),
            low_balance_config,
            notified_streams: Arc::new(Mutex::new(HashSet::new())),
        };

        Ok(overseer)
    }

    pub fn database(&self) -> ZapStreamDb {
        self.db.clone()
    }

    #[cfg(feature = "zap-stream")]
    pub fn lnd_client(&self) -> fedimint_tonic_lnd::Client {
        self.lnd.clone()
    }

    pub fn stream_manager(&self) -> StreamManager {
        self.stream_manager.clone()
    }

    pub fn nostr_client(&self) -> Client {
        self.client.clone()
    }

    async fn stream_to_event_builder(&self, stream: &UserStream) -> Result<EventBuilder> {
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
            let viewer_count = self.stream_manager.get_viewer_count(&stream.id).await;
            tags.push(Tag::parse(&[
                "current_participants".to_string(),
                viewer_count.to_string(),
            ])?);
        }

        let signer = self.client.signer().await?;
        let coord =
            Coordinate::new(Kind::LiveEvent, signer.get_public_key().await?).identifier(&stream.id);
        tags.push(Tag::parse([
            "alt",
            &format!(
                "Watch live on https://zap.stream/{}",
                nostr_sdk::nips::nip19::Nip19Coordinate {
                    coordinate: coord,
                    relays: vec![]
                }
                .to_bech32()?
            ),
        ])?);
        Ok(EventBuilder::new(Kind::LiveEvent, "").tags(tags))
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
                self.map_to_public_url(
                    pipeline_dir
                        .join(format!("thumb.webp?n={}", Utc::now().timestamp()))
                        .to_str()
                        .unwrap(),
                )?
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
        let ev = self.stream_to_event_builder(stream).await?.tags(extra_tags);
        let ev = self.client.sign_event_builder(ev).await?;
        self.client.send_event(&ev).await?;
        info!("Published stream event {}", ev.id.to_hex());
        Ok(ev)
    }

    fn map_to_public_url(&self, path: &str) -> Result<String> {
        let u: Url = self.public_url.parse()?;
        Ok(u.join(path)?.to_string())
    }

    /// Send low balance notification to user and admin
    async fn send_low_balance_notification(&self, user_id: u64, user_pubkey: &[u8], current_balance: i64, stream_id: &Uuid) -> Result<()> {
        if let Some(config) = &self.low_balance_config {
            let balance_sats = current_balance / 1000; // Convert millisats to sats
            let message = format!(
                "⚠️ Low Balance Warning ⚠️\n\nYour streaming balance is low: {} sats\n\nPlease top up your account to avoid stream interruption.\nStream ID: {}",
                balance_sats, stream_id
            );

            // Send DM to user
            if let Ok(user_pk) = PublicKey::from_slice(user_pubkey) {
                match EventBuilder::encrypted_direct_msg(&self.client.signer().await?, &user_pk, &message, None) {
                    Ok(dm_builder) => {
                        match self.client.send_event_builder(dm_builder).await {
                            Ok(_) => info!("Sent low balance notification to user {}", user_id),
                            Err(e) => warn!("Failed to send low balance notification to user {}: {}", user_id, e),
                        }
                    },
                    Err(e) => warn!("Failed to create low balance notification for user {}: {}", user_id, e),
                }
            }

            // Send notification to admin if configured
            if let Ok(admin_pk) = PublicKey::from_hex(&config.admin_pubkey) {
                let admin_message = format!(
                    "Low Balance Alert\n\nUser: {}\nBalance: {} sats\nStream: {}\n\nUser may need assistance with topping up their account.",
                    hex::encode(user_pubkey), balance_sats, stream_id
                );
                match EventBuilder::encrypted_direct_msg(&self.client.signer().await?, &admin_pk, &admin_message, None) {
                    Ok(admin_dm_builder) => {
                        match self.client.send_event_builder(admin_dm_builder).await {
                            Ok(_) => info!("Sent low balance alert to admin for user {}", user_id),
                            Err(e) => warn!("Failed to send low balance alert to admin for user {}: {}", user_id, e),
                        }
                    },
                    Err(e) => warn!("Failed to create low balance alert for admin for user {}: {}", user_id, e),
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Overseer for ZapStreamOverseer {
    async fn check_streams(&self) -> Result<()> {
        let active_streams = self.db.list_live_streams().await?;

        for stream in active_streams {
            // check if stream is alive
            let id = Uuid::parse_str(&stream.id)?;
            info!("Checking stream is alive: {}", stream.id);

            let (is_active, should_timeout) =
                self.stream_manager.check_stream_status(&stream.id).await;

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
                    if self
                        .stream_manager
                        .check_and_update_viewer_count(&stream.id)
                        .await?
                    {
                        self.publish_stream_event(&stream, &user.pubkey).await?;
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
        let endpoint = self.detect_endpoint(connection).await?;

        let caps = parse_capabilities(&endpoint.capabilities);
        let cfg = get_variants_from_endpoint(stream_info, &caps)?;

        if cfg.video_src.is_none() || cfg.variants.is_empty() {
            bail!("No video src found");
        }

        let mut egress = vec![];
        let all_var_ids: HashSet<Uuid> = cfg.variants.iter().map(|v| v.id()).collect();
        egress.push(EgressType::HLS(
            all_var_ids.clone(),
            self.segment_length,
            SegmentType::FMP4,
        ));
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
                    egress.push(EgressType::Recorder(
                        vars_in_group.map(|v| v.id()).collect(),
                    ))
                }
                None => {
                    warn!(
                        "Invalid DVR config, no variant found with height {}",
                        height
                    );
                }
            }
        }

        let forward_dest = self.db.get_user_forwards(user.id).await?;
        for fwd in forward_dest {
            egress.push(EgressType::RTMPForwarder(all_var_ids.clone(), fwd.target));
        }

        let stream_id = connection.id;
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
            .add_active_stream(
                &new_stream.id,
                cfg.video_src.map(|s| s.fps).unwrap(),
                &endpoint.name,
                cfg.video_src
                    .map(|s| format!("{}x{}", s.width, s.height))
                    .unwrap()
                    .as_str(),
                connection.endpoint,
                &connection.ip_addr,
            )
            .await;

        self.db.insert_stream(&new_stream).await?;
        self.db.update_stream(&new_stream).await?;

        // publish N94 stream
        if let Some(n94) = &self.n94 {
            n94.on_start(N94StreamInfo {
                id: new_stream.id.clone(),
                title: new_stream.title.clone(),
                summary: new_stream.summary.clone(),
                image: new_stream.image.clone(),
                tags: vec![],
                starts: new_stream.starts.timestamp() as _,
                ends: None,
                relays: vec![],
                variants: cfg
                    .variants
                    .chunk_by(|a, b| a.group_id() == b.group_id())
                    .map_while(|v| {
                        let video = v.iter().find_map(|a| match a {
                            VariantStream::Video(v) | VariantStream::CopyVideo(v) => Some(v),
                            _ => None,
                        });
                        let video = if let Some(v) = video {
                            v
                        } else {
                            return None;
                        };
                        Some(N94Variant {
                            id: video.id().to_string(),
                            width: video.width as _,
                            height: video.height as _,
                            bitrate: video.bitrate as _,
                            mime_type: Some("video/mp2t".to_string()),
                        })
                    })
                    .collect(),
                goal: new_stream.goal.clone(),
                pinned: new_stream.pinned.clone(),
                status: "live".to_string(),
            })
            .await?;
        }
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

        // Check for low balance and send notification if needed
        if let Some(config) = &self.low_balance_config {
            if bal > 0 && bal <= config.threshold_msats {
                // Check if we haven't already notified for this stream
                let mut notified = self.notified_streams.lock().await;
                if !notified.contains(pipeline_id) {
                    // Get user info for notification
                    if let Ok(user) = self.db.get_user(stream.user_id).await {
                        if let Err(e) = self.send_low_balance_notification(
                            stream.user_id, 
                            &user.pubkey, 
                            bal, 
                            pipeline_id
                        ).await {
                            warn!("Failed to send low balance notification: {}", e);
                        }
                    }
                    notified.insert(*pipeline_id);
                }
            }
        }

        if bal <= 0 {
            bail!("Balance has run out");
        }

        // Update last segment time for this stream
        self.stream_manager
            .update_stream_segment_time(&stream.id)
            .await;

        if let Some(n94) = &self.n94 {
            n94.on_new_segment(added.iter().map(|s| into_n94_segment(s)).collect())
                .await?;
            n94.on_deleted_segment(deleted.iter().map(|s| into_n94_segment(s)).collect())
                .await?;
        }
        Ok(())
    }

    async fn on_thumbnail(
        &self,
        _pipeline_id: &Uuid,
        _width: usize,
        _height: usize,
        _pixels: &PathBuf,
    ) -> Result<()> {
        // nothing to do
        Ok(())
    }

    async fn on_end(&self, pipeline_id: &Uuid) -> Result<()> {
        let mut stream = self.db.get_stream(pipeline_id).await?;
        let user = self.db.get_user(stream.user_id).await?;

        self.stream_manager.remove_active_stream(&stream.id).await;

        // Clean up notification tracking for this stream
        {
            let mut notified = self.notified_streams.lock().await;
            notified.remove(pipeline_id);
        }

        stream.state = UserStreamState::Ended;
        stream.ends = Some(Utc::now());
        let event = self.publish_stream_event(&stream, &user.pubkey).await?;
        stream.event = Some(event.as_json());
        self.db.update_stream(&stream).await?;

        info!("Stream ended {}", stream.id);
        Ok(())
    }

    async fn on_update(&self, pipeline_id: &Uuid) -> Result<()> {
        let mut stream = self.db.get_stream(pipeline_id).await?;
        let user = self.db.get_user(stream.user_id).await?;

        let event = self.publish_stream_event(&stream, &user.pubkey).await?;
        stream.event = Some(event.as_json());
        self.db.update_stream(&stream).await?;
        Ok(())
    }

    async fn on_stats(&self, pipeline_id: &Uuid, stats: StatsType) -> Result<()> {
        let id = pipeline_id.to_string();
        match stats {
            StatsType::Ingress(i) | StatsType::Egress(i) => {
                self.stream_manager.update_endpoint_metrics(&id, i).await;
            }
            StatsType::Pipeline(p) => {
                self.stream_manager
                    .update_pipeline_metrics(&id, p.average_fps, p.total_frames)
                    .await;
            }
        }

        Ok(())
    }
}

impl ZapStreamOverseer {
    /// Detect which ingest endpoint should be used based on connection info
    async fn detect_endpoint(&self, connection: &ConnectionInfo) -> Result<IngestEndpoint> {
        let endpoints = self.db.get_ingest_endpoints().await?;

        if endpoints.is_empty() {
            bail!("No endpoints found, please configure endpoints first!");
        }
        let default = endpoints.iter().max_by_key(|e| e.cost);
        Ok(endpoints
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(connection.endpoint))
            .or(default)
            .unwrap()
            .clone())
    }
}

fn into_n94_segment(seg: &EgressSegment) -> N94Segment {
    N94Segment {
        variant: seg.variant.to_string(),
        idx: seg.idx,
        duration: seg.duration,
        path: seg.path.clone(),
        sha256: seg.sha256.clone(),
    }
}
