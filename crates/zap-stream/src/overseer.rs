use crate::payments::create_lightning;
use crate::settings::{AdvertiseConfig, PaymentBackend, RedisConfig, Settings};
use crate::stream_manager::StreamManager;
use anyhow::{Context, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use nostr_sdk::prelude::Coordinate;
use nostr_sdk::{
    Client, Event, EventBuilder, JsonUtil, Keys, Kind, Metadata, NostrSigner, Tag, Timestamp,
    ToBech32,
};
use nwc::NWC;
use nwc::prelude::{NostrWalletConnectURI, PayInvoiceRequest};
use payments_rs::lightning::{AddInvoiceRequest, InvoiceUpdate, LightningNode};
use std::collections::{HashMap, HashSet};
use std::ops::Add;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs::remove_dir_all;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use url::Url;
use uuid::Uuid;
use zap_stream_core::egress::EgressSegment;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::egress::recorder::RecorderEgress;
use zap_stream_core::endpoint::{
    EndpointCapability, get_variants_from_endpoint, parse_capabilities,
};
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_core::mux::SegmentType;
use zap_stream_core::overseer::{ConnectResult, IngressInfo, Overseer, StatsType};
use zap_stream_core::pipeline::{EgressType, PipelineConfig};
use zap_stream_core::variant::{StreamMapping, VariantStream};
use zap_stream_core_nostr::n94::{N94Publisher, N94Segment, N94StreamInfo, N94Variant};
use zap_stream_db::{
    IngestEndpoint, Payment, StreamKeyType, User, UserStream, UserStreamState, ZapStreamDb,
};

#[cfg(feature = "moq")]
use zap_stream_core::hang::moq_lite::{OriginConsumer, OriginProducer, Produce};

/// zap.stream NIP-53 overseer
#[derive(Clone)]
pub struct ZapStreamOverseer {
    /// Database instance for accounts/streams
    db: ZapStreamDb,
    /// Generic node backend
    lightning: Arc<dyn LightningNode + Send + Sync>,
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
    /// Low balance notification threshold in sats
    low_balance_threshold: Option<u64>,
    /// Node name for horizontal scaling
    node_name: String,
    /// NWC topup handles
    nwc_topup_requests: Arc<RwLock<HashMap<u64, JoinHandle<()>>>>,
    /// Primary output directory for media
    out_dir: PathBuf,
    #[cfg(feature = "moq")]
    moq_origin: Option<Arc<Produce<OriginProducer, OriginConsumer>>>,
}

impl ZapStreamOverseer {
    /// Time window when we allow resuming a recently ended stream on the primary key
    const RECONNECT_WINDOW_SECONDS: u64 = 120;

    pub async fn from_settings(settings: &Settings, shutdown: CancellationToken) -> Result<Self> {
        ZapStreamOverseer::new(
            &settings.public_url,
            &settings.overseer.nsec,
            &settings.overseer.database,
            &settings.overseer.payments,
            &settings.overseer.relays,
            &settings.overseer.blossom,
            settings.overseer.segment_length.unwrap_or(2.0),
            settings.overseer.low_balance_threshold,
            &settings.overseer.advertise,
            &settings.redis,
            PathBuf::from(&settings.output_dir),
            shutdown,
        )
        .await
    }

    pub async fn new(
        public_url: &String,
        private_key: &str,
        db: &str,
        payments: &PaymentBackend,
        relays: &Vec<String>,
        blossom_servers: &Option<Vec<String>>,
        segment_length: f32,
        low_balance_threshold: Option<u64>,
        advertise: &Option<AdvertiseConfig>,
        redis: &Option<RedisConfig>,
        out_dir: PathBuf,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        let db = ZapStreamDb::new(db).await?;
        db.migrate().await?;

        #[cfg(debug_assertions)]
        {
            let uid = db.upsert_user(&[0; 32]).await?;
            let z_user = db.get_user(uid).await?;
            if z_user.balance == 0 {
                db.update_user_balance(uid, 100_000_000).await?;
            }

            info!(
                "ZERO pubkey: uid={},key={},balance={}",
                z_user.id,
                z_user.stream_key,
                z_user.balance / 1000
            );
        }

        let payments = create_lightning(payments, db.clone()).await?;
        let keys = Keys::from_str(private_key)?;
        let client = nostr_sdk::ClientBuilder::new().signer(keys.clone()).build();
        for r in relays {
            client.add_relay(r).await?;
        }
        client.connect().await;

        let node_name = sysinfo::System::host_name()
            .ok_or_else(|| anyhow::anyhow!("Failed to get hostname!"))?;

        // Initialize StreamManager with Redis if available
        let (stream_manager, redis_client) = if let Some(r) = redis {
            let r_client = redis::Client::open(r.url.clone())?;
            (
                StreamManager::new_with_redis(node_name.clone(), r_client.clone()).await?,
                Some(r_client),
            )
        } else {
            (StreamManager::new(node_name.clone()), None)
        };

        let mut overseer = Self {
            db,
            lightning: payments,
            n94: blossom_servers
                .as_ref()
                .map(|s| N94Publisher::new(client.clone(), s, 3, segment_length)),
            client: client.clone(),
            segment_length,
            public_url: public_url.clone(),
            stream_manager,
            low_balance_threshold,
            node_name,
            nwc_topup_requests: Arc::new(RwLock::new(HashMap::new())),
            out_dir,
            #[cfg(feature = "moq")]
            moq_origin: None,
        };

        // Enable Redis stats distribution if available
        if let Some(r_client) = redis_client {
            let _ = overseer
                .stream_manager
                .enable_redis(r_client, shutdown.clone())
                .await;
        }
        let _ = overseer.stream_manager.start_cleanup_task(shutdown.clone());
        let _ = overseer.stream_manager.start_node_metrics_task(shutdown);

        // advertise self via NIP-89
        if let Some(a) = advertise {
            let meta = Metadata {
                name: a.name.clone(),
                display_name: None,
                about: a.about.clone(),
                website: Some(overseer.map_to_public_url("api/v1")?.to_string()),
                picture: a.picture.clone(),
                ..Default::default()
            };
            let app = EventBuilder::new(Kind::Custom(31_990), meta.as_json())
                .tag(Tag::identifier(
                    a.id.as_deref().unwrap_or("zap-stream-core"),
                ))
                .tag(Tag::parse(["k", "30311"])?)
                .tag(Tag::parse(["i", "api:zap-stream"])?);
            info!("Advertising app handler: {}", meta.as_json());
            client.send_event_builder(app).await?;
        }

        Ok(overseer)
    }

    pub fn database(&self) -> ZapStreamDb {
        self.db.clone()
    }

    pub fn lightning(&self) -> Arc<dyn LightningNode + Send + Sync> {
        self.lightning.clone()
    }

    #[cfg(feature = "moq")]
    pub fn set_moq_origin(&mut self, origin: Arc<Produce<OriginProducer, OriginConsumer>>) {
        self.moq_origin = Some(origin);
    }

    pub fn start_payment_handler(&self, token: CancellationToken) -> JoinHandle<Result<()>> {
        let ln = self.lightning.clone();
        let db = self.db.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            loop {
                // get last completed payment
                let last_payment_hash = if let Some(pl) = db.get_latest_completed_payment().await? {
                    Some(pl.payment_hash)
                } else {
                    None
                };
                info!(
                    "Listening to invoices from {}",
                    last_payment_hash
                        .as_ref()
                        .map(hex::encode)
                        .unwrap_or("Now".to_string())
                );
                let mut stream = ln.subscribe_invoices(last_payment_hash).await?;

                loop {
                    tokio::select! {
                        _ = token.cancelled() => {
                            info!("Payment handler exiting...");
                            return Ok(());
                        }
                        Some(msg) = stream.next() => {
                            //info!("Received message: {:?}", msg);
                            match msg {
                               InvoiceUpdate::Settled {
                                    payment_hash, preimage, ..
                                } => {
                                    if let Err(e) = Self::try_complete_payment(payment_hash, preimage, &db, &client).await {
                                        error!("Failed to complete payment: {}", e);
                                    }
                                }
                                InvoiceUpdate::Error(error) => {
                                    error!("Invoice update error: {}", error);
                                }
                                _ => {}
                            }
                        }
                    }
                }

                info!("Payment handler exiting, resetting...");
            }
            Ok(())
        })
    }

    async fn try_complete_payment(
        payment_hash: String,
        pre_image: Option<String>,
        db: &ZapStreamDb,
        client: &Client,
    ) -> Result<()> {
        let ph = hex::decode(&payment_hash)?;
        match db.complete_payment(&ph, 0).await {
            Ok(b) => {
                if b {
                    info!("Completed payment!");
                    let payment = db.get_payment(&ph).await?.unwrap();
                    if let Some(nostr) = payment.nostr {
                        Self::try_send_zap_receipt(
                            client,
                            &nostr,
                            payment
                                .invoice
                                .ok_or(anyhow!("invoice was empty"))?
                                .as_str(),
                            pre_image,
                        )
                        .await?;
                    }
                } else {
                    warn!("No payments updated! Maybe it doesnt exist or it's already processed.")
                }
            }
            Err(e) => {
                error!("Failed to complete payment {}: {}", payment_hash, e);
            }
        }
        Ok(())
    }

    async fn try_send_zap_receipt(
        client: &Client,
        zap_request: &str,
        invoice: &str,
        pre_image: Option<String>,
    ) -> Result<()> {
        let ev = Event::from_json(zap_request)?;
        ensure!(ev.kind == Kind::ZapRequest, "Wrong zap request kind");
        ensure!(ev.verify().is_ok(), "Invalid zap request sig");

        let copy_tags = ev
            .tags
            .iter()
            .filter(|t| t.single_letter_tag().is_some())
            .cloned();
        let mut receipt = EventBuilder::new(Kind::ZapReceipt, "")
            .tags(copy_tags)
            .tag(Tag::description(zap_request))
            .tag(Tag::parse(["bolt11", invoice])?);
        if let Some(r) = pre_image {
            receipt = receipt.tag(Tag::parse(["preimage", &r])?);
        }

        let id = client.send_event_builder(receipt).await?;
        info!("Sent zap receipt {}", id.val);
        Ok(())
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
        if let Some(ref title) = stream.title
            && !title.trim().is_empty()
        {
            tags.push(Tag::parse(&["title".to_string(), title.to_string()])?);
        }
        if let Some(ref summary) = stream.summary
            && !summary.trim().is_empty()
        {
            tags.push(Tag::parse(&["summary".to_string(), summary.to_string()])?);
        }
        if let Some(ref image) = stream.image
            && !image.trim().is_empty()
        {
            tags.push(Tag::parse(&["image".to_string(), image.to_string()])?);
        }
        if let Some(ref thumb) = stream.thumb
            && !thumb.trim().is_empty()
        {
            tags.push(Tag::parse(&["thumb".to_string(), thumb.to_string()])?);
        }
        if let Some(ref content_warning) = stream.content_warning
            && !content_warning.trim().is_empty()
        {
            tags.push(Tag::parse(&[
                "content_warning".to_string(),
                content_warning.to_string(),
            ])?);
        }
        if let Some(ref goal) = stream.goal
            && !goal.trim().is_empty()
        {
            tags.push(Tag::parse(&["goal".to_string(), goal.to_string()])?);
        }
        if let Some(ref pinned) = stream.pinned
            && !pinned.trim().is_empty()
        {
            tags.push(Tag::parse(&["pinned".to_string(), pinned.to_string()])?);
        }
        if let Some(ref tags_csv) = stream.tags {
            for tag in tags_csv.split(',') {
                if tag.trim().is_empty() {
                    continue;
                }
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

        let mut eb = EventBuilder::new(Kind::LiveEvent, "").tags(tags);

        // make sure this event is always newer
        if let Some(previous_event) = &stream.event
            && let Ok(prev_event) = Event::from_json(previous_event)
            && prev_event.created_at >= Timestamp::now()
        {
            eb = eb.custom_created_at(prev_event.created_at.add(Timestamp::from_secs(1)));
        }

        Ok(eb)
    }

    pub async fn publish_stream_event(
        &self,
        stream: &UserStream,
        pubkey: &Vec<u8>,
    ) -> Result<Event> {
        let pipeline_dir = PathBuf::from(stream.id.to_string());
        let mut extra_tags = vec![
            Tag::parse(["p", hex::encode(pubkey).as_str(), "", "host"])?,
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

    fn map_to_public_url(&self, path: &str) -> Result<Url> {
        let u: Url = self.public_url.parse()?;
        Ok(u.join(path)?)
    }

    /// Send low balance notification as live chat message
    async fn send_low_balance_notification(&self, user: &User, stream: &UserStream) -> Result<()> {
        if self.low_balance_threshold.is_some() {
            let balance_sats = user.balance / 1000; // Convert millisats to sats
            let message = format!(
                "⚠️ Low Balance Warning ⚠️ Your streaming balance is low: {} sats. Please top up your account to avoid stream interruption.",
                balance_sats
            );

            // Send live chat message to the stream
            let stream_event = if let Some(e) = &stream.event {
                Event::from_json(e)?
            } else {
                bail!("Cannot send low balance notification, stream event json is empty!")
            };

            let chat_event = EventBuilder::new(Kind::Custom(1311), message).tag(Tag::coordinate(
                stream_event
                    .coordinate()
                    .context("stream event json invalid")?
                    .into_owned(),
                None,
            ));

            match self.client.send_event_builder(chat_event).await {
                Ok(_) => info!(
                    "Sent low balance notification to stream {} for user {}",
                    stream.id, user.id
                ),
                Err(e) => warn!(
                    "Failed to send low balance notification to stream {} for user {}: {}",
                    stream.id, user.id, e
                ),
            }
        }
        Ok(())
    }

    async fn get_user_key(&self, info: &ConnectionInfo) -> Result<(StreamKeyType, User)> {
        let user_key = self
            .db
            .find_user_stream_key(&info.key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found or invalid stream key"))?;

        let uid = match user_key {
            StreamKeyType::Primary(i) => i,
            StreamKeyType::FixedEventKey { id, .. } => id,
        };
        let user = self.db.get_user(uid).await?;
        Ok((user_key, user))
    }

    pub async fn topup(
        &self,
        pubkey: &[u8; 32],
        amount_msats: u64,
        nostr: Option<String>,
    ) -> Result<Payment> {
        let uid = self.db.upsert_user(pubkey).await?;

        let response = self
            .lightning
            .add_invoice(AddInvoiceRequest {
                amount: amount_msats,
                memo: Some(format!("zap.stream topup for user {}", hex::encode(pubkey))),
                expire: None,
            })
            .await?;

        let r_hash = hex::decode(response.payment_hash())?;
        // Create payment entry for this topup invoice
        self.db
            .create_payment(
                &r_hash,
                uid,
                Some(&response.pr()),
                amount_msats,
                zap_stream_db::PaymentType::TopUp,
                0,
                DateTime::from_timestamp(
                    response.parsed_invoice.expires_at().unwrap().as_secs() as _,
                    0,
                )
                .unwrap(),
                nostr,
                response.external_id,
            )
            .await?;

        let payment = self.db.get_payment(&r_hash).await?;
        Ok(payment.unwrap())
    }
}

#[async_trait]
impl Overseer for ZapStreamOverseer {
    async fn check_streams(&self) -> Result<()> {
        let active_streams = self.db.list_live_streams_by_node(&self.node_name).await?;

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
                if let Ok(user) = self.db.get_user(stream.user_id).await
                    && self
                        .stream_manager
                        .check_and_update_viewer_count(&stream.id)
                        .await?
                {
                    self.publish_stream_event(&stream, &user.pubkey).await?;
                }
            }
        }

        let recently_ended = self
            .db
            .list_recently_ended_streams_by_node(&self.node_name)
            .await?;
        for stream in recently_ended {
            let should_cleanup = if let Some(e) = &stream.ends
                // ends in the past
                && *e < Utc::now()
                // reconnect window expired
                && Utc::now().timestamp().abs_diff(e.timestamp()) > Self::RECONNECT_WINDOW_SECONDS
            {
                true
            } else {
                false
            };

            if !should_cleanup {
                continue;
            }
            let out_dir_hls = self.out_dir.join(stream.id).join(HlsEgress::PATH);
            if out_dir_hls.exists() {
                info!("Deleting expired HLS stream data {}", out_dir_hls.display());
                if let Err(e) = remove_dir_all(&out_dir_hls).await {
                    warn!("Failed to delete expired HLS stream data: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn connect(&self, connection_info: &ConnectionInfo) -> Result<ConnectResult> {
        let (user_key, user) = self.get_user_key(connection_info).await?;
        let hex_pubkey = hex::encode(&user.pubkey);

        let endpoint = self.detect_endpoint(connection_info).await?;
        if user.balance <= 0 && endpoint.cost != 0 {
            return Ok(ConnectResult::Deny {
                reason: format!(
                    "Not enough balance pubkey={}, balance={}",
                    hex_pubkey, user.balance
                ),
            });
        }

        if user.is_blocked {
            return Ok(ConnectResult::Deny {
                reason: format!("User is blocked! pubkey={}", hex_pubkey),
            });
        }

        let prev_streams = self.db.get_user_prev_streams(user.id).await?;

        // reject multiple live streams for the same primary key
        if matches!(user_key, StreamKeyType::Primary(_)) && prev_streams.live_primary_count != 0 {
            return Ok(ConnectResult::Deny {
                reason: "Primary key is already in use, please check if you are already live!"
                    .to_string(),
            });
        }

        // check if the user is not live right now and has a stream that ended in the past 2mins
        // otherwise we will resume the previous stream event
        let has_recent_stream = prev_streams.live_primary_count == 0
            && prev_streams
                .last_ended
                .map(|v| {
                    v.timestamp().abs_diff(Utc::now().timestamp()) < Self::RECONNECT_WINDOW_SECONDS
                })
                .unwrap_or(false);

        Ok(ConnectResult::Allow {
            enable_stream_dump: user.stream_dump_recording,
            stream_id_override: match (has_recent_stream, user_key) {
                (true, StreamKeyType::Primary(_)) => {
                    let prev_id = prev_streams
                        .last_stream_id
                        .and_then(|id| id.parse().ok())
                        .ok_or(anyhow!(
                            "Expected previous stream key not found, or could not be parsed!"
                        ))?;
                    info!("Resuming previous stream {}", prev_id);
                    Some(prev_id)
                }
                (_, StreamKeyType::FixedEventKey { stream_id, .. }) => Some(
                    stream_id
                        .parse()
                        .map_err(|e| anyhow!("Failed to parse fixed stream id: {}", e))?,
                ),
                _ => None,
            },
        })
    }

    async fn start_stream(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> Result<PipelineConfig> {
        let (user_key, user) = self.get_user_key(connection).await?;
        let hex_pubkey = hex::encode(&user.pubkey);
        let uid = user.id;

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

        // let forward_dest = self.db.get_user_forwards(user.id).await?;
        // for fwd in forward_dest {
        //     egress.push(EgressType::RTMPForwarder(all_var_ids.clone(), fwd.target));
        // }

        // in cases where the previous stream should be resumed, the pipeline ID will match a previous
        // stream so we should first try to find the current pipeline id as if it already exists
        let stream_id = connection.id;
        let prev_stream = self.db.try_get_stream(&stream_id).await?;

        let mut new_stream = match &user_key {
            StreamKeyType::Primary(_) => {
                // resume previously ended stream
                if let Some(mut prev_stream) = prev_stream {
                    prev_stream.state = UserStreamState::Live;
                    prev_stream.node_name = Some(self.node_name.clone());
                    prev_stream.endpoint_id = Some(endpoint.id);
                    prev_stream.ends = None;
                    prev_stream
                } else {
                    // start a new stream
                    let new_stream = UserStream {
                        id: stream_id.to_string(),
                        user_id: uid,
                        starts: Utc::now(),
                        state: UserStreamState::Live,
                        endpoint_id: Some(endpoint.id),
                        title: user.title.clone(),
                        summary: user.summary.clone(),
                        image: user.image.clone(),
                        content_warning: user.content_warning.clone(),
                        goal: user.goal.clone(),
                        tags: user.tags.clone(),
                        node_name: Some(self.node_name.clone()),
                        ..Default::default()
                    };

                    self.db.insert_stream(&new_stream).await?;
                    new_stream
                }
            }
            StreamKeyType::FixedEventKey { stream_id, .. } => {
                let stream_uuid = Uuid::parse_str(stream_id)
                    .map_err(|e| anyhow!("Invalid stream key {} {}", stream_id, e))?;
                let mut stream = self.db.get_stream(&stream_uuid).await?;

                stream.state = UserStreamState::Live;
                stream.node_name = Some(self.node_name.clone());
                stream.endpoint_id = Some(endpoint.id);
                stream.ends = None;
                stream
            }
        };

        self.stream_manager
            .add_active_stream(
                &hex_pubkey,
                user.id,
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

        let stream_event = self.publish_stream_event(&new_stream, &user.pubkey).await?;
        new_stream.event = Some(stream_event.as_json());
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
                        let video = video?;
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
        let stream = self
            .db
            .get_stream(pipeline_id)
            .await
            .context("Failed to find stream")?;

        // Get the cost per minute from the ingest endpoint, or use default
        let endpoint = if let Some(endpoint_id) = stream.endpoint_id {
            self.db.get_ingest_endpoint(endpoint_id).await?
        } else {
            bail!("Endpoint id not set on stream");
        };

        let (duration, cost) = get_cost(&endpoint, added);
        let bal = self
            .db
            .tick_stream(pipeline_id, stream.user_id, duration, cost)
            .await?;

        if cost > 0 {
            let user = self
                .db
                .get_user(stream.user_id)
                .await
                .context("Failed to get user")?;
            // try to auto-topup with NWC when balance is below 1000 sats
            const NWC_TOPUP_AMOUNT: u64 = 1000_000;
            if user.balance < NWC_TOPUP_AMOUNT as _ && user.nwc.is_some() {
                let has_task = { self.nwc_topup_requests.read().await.contains_key(&user.id) };
                if !has_task {
                    let user = user.clone();
                    let overseer = self.clone();
                    let jh = tokio::spawn(async move {
                        let nwc_url = match NostrWalletConnectURI::parse(user.nwc.unwrap()) {
                            Ok(u) => u,
                            Err(e) => {
                                error!("Failed to parse NWC url for user {}: {}", user.id, e);
                                overseer.nwc_topup_requests.write().await.remove(&user.id);
                                return;
                            }
                        };
                        let nwc = NWC::new(nwc_url);

                        let pubkey = user.pubkey.as_slice().try_into().unwrap();
                        let topup = match overseer.topup(pubkey, NWC_TOPUP_AMOUNT, None).await {
                            Ok(v) => v,
                            Err(e) => {
                                error!("Failed to get topup for user {}: {}", user.id, e);
                                overseer.nwc_topup_requests.write().await.remove(&user.id);
                                return;
                            }
                        };

                        let pr = if let Some(pr) = topup.invoice {
                            pr
                        } else {
                            error!("Cannot make payment, invoice was null");
                            overseer.nwc_topup_requests.write().await.remove(&user.id);
                            return;
                        };
                        match nwc
                            .pay_invoice(PayInvoiceRequest {
                                id: None,
                                invoice: pr,
                                amount: None,
                            })
                            .await
                        {
                            Ok(p) => {
                                info!(
                                    "NWC auto-topup complete for user {} preimage={}, fees={}",
                                    user.id,
                                    p.preimage,
                                    p.fees_paid.unwrap_or(0)
                                );
                            }
                            Err(e) => error!("Failed to pay invoice for user {}: {}", user.id, e),
                        }
                        overseer.nwc_topup_requests.write().await.remove(&user.id);
                    });
                    self.nwc_topup_requests.write().await.insert(user.id, jh);
                    info!("Starting NWC topup for {}", user.id);
                }
            }

            // Check for low balance and send notification if needed
            if let Some(threshold) = self.low_balance_threshold {
                let threshold = threshold as i64 * 1000; // convert to msats
                let balance_before = bal + cost; // Calculate balance before this deduction
                if balance_before > threshold && bal <= threshold {
                    // Balance just crossed the threshold, send notification
                    if let Err(e) = self.send_low_balance_notification(&user, &stream).await {
                        warn!("Failed to send low balance notification: {}", e);
                    }
                }
            }

            if bal <= 0 {
                bail!("Balance has run out");
            }
        }

        // Update last segment time for this stream
        self.stream_manager
            .update_stream_segment_time(&stream.id)
            .await;

        if let Some(n94) = &self.n94 {
            n94.on_new_segment(added.iter().map(into_n94_segment).collect())
                .await?;
            n94.on_deleted_segment(deleted.iter().map(into_n94_segment).collect())
                .await?;
        }
        Ok(())
    }

    async fn on_thumbnail(
        &self,
        pipeline_id: &Uuid,
        _width: usize,
        _height: usize,
        _pixels: &PathBuf,
    ) -> Result<()> {
        let pipeline_dir = PathBuf::from(pipeline_id.to_string());

        let mut stream = self.db.get_stream(pipeline_id).await?;

        let thumb_url = self.map_to_public_url(
            pipeline_dir
                .join(format!("thumb.webp?n={}", Utc::now().timestamp()))
                .to_str()
                .unwrap(),
        )?;
        stream.thumb = Some(thumb_url.to_string());
        self.db.update_stream(&stream).await?;

        Ok(())
    }

    async fn on_end(&self, pipeline_id: &Uuid) -> Result<()> {
        let mut stream = self.db.get_stream(pipeline_id).await?;
        let user = self.db.get_user(stream.user_id).await?;

        self.stream_manager.remove_active_stream(&stream.id).await;

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

    #[cfg(feature = "moq")]
    async fn get_moq_origin(&self) -> Result<OriginProducer> {
        let Some(prod) = &self.moq_origin else {
            bail!("MoQ not configured")
        };
        Ok(prod.producer.clone())
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
            .find(|e| e.name.eq_ignore_ascii_case(&connection.app_name))
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
        sha256: seg.sha256,
    }
}

fn get_cost(endpoint: &IngestEndpoint, segments: &[EgressSegment]) -> (f32, i64) {
    // count total duration from all segments (including copied)
    let duration = segments.iter().fold(0.0, |acc, v| acc + v.duration);

    let cost_per_minute = endpoint.cost;

    // Convert duration from seconds to minutes and calculate cost
    let duration_minutes = duration / 60.0;

    // cost can never be negative
    let cost = (cost_per_minute as f32 * duration_minutes).round().max(0.0);
    let cost = if cost.is_normal() { cost as i64 } else { 0 };

    // Ensure duration is also non-negative
    let duration = duration.max(0.0);

    (duration, cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn create_test_endpoint(cost: u64) -> IngestEndpoint {
        IngestEndpoint {
            id: 1,
            name: "test_endpoint".to_string(),
            cost,
            capabilities: None,
        }
    }

    fn create_test_segment(duration: f32) -> EgressSegment {
        EgressSegment {
            variant: Uuid::new_v4(),
            idx: 0,
            duration,
            path: PathBuf::from("/test"),
            sha256: [0u8; 32],
        }
    }

    #[test]
    fn test_get_cost_positive_values() {
        let endpoint = create_test_endpoint(1000); // 1000 millisats per minute
        let segments = vec![
            create_test_segment(60.0), // 1 minute
            create_test_segment(30.0), // 30 seconds
        ];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        assert_eq!(duration, 90.0);
        assert_eq!(cost, 1500); // 1.5 minutes * 1000 millisats/minute
    }

    #[test]
    fn test_get_cost_zero_cost_endpoint() {
        let endpoint = create_test_endpoint(0); // Free endpoint
        let segments = vec![create_test_segment(120.0)];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        assert_eq!(duration, 120.0);
        assert_eq!(cost, 0);
    }

    #[test]
    fn test_get_cost_zero_duration() {
        let endpoint = create_test_endpoint(1000);
        let segments = vec![create_test_segment(0.0)];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        assert_eq!(duration, 0.0);
        assert_eq!(cost, 0);
    }

    #[test]
    fn test_get_cost_empty_segments() {
        let endpoint = create_test_endpoint(1000);
        let segments = vec![];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        assert_eq!(duration, 0.0);
        assert_eq!(cost, 0);
    }

    #[test]
    fn test_get_cost_all_segments_counted() {
        let endpoint = create_test_endpoint(1000);
        let segments = vec![
            create_test_segment(60.0), // 1 minute
            create_test_segment(60.0), // 1 minute
        ];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        // All segments should count
        assert_eq!(duration, 120.0);
        assert_eq!(cost, 2000); // 2 minutes * 1000 millisats/minute
    }

    #[test]
    fn test_get_cost_negative_duration_handling() {
        let endpoint = create_test_endpoint(1000);
        // Test with negative duration (edge case that shouldn't happen in practice)
        let segments = vec![create_test_segment(-60.0)];

        let (duration, cost) = get_cost(&endpoint, &segments);

        // Even with invalid input, output should never be negative
        assert!(
            duration >= 0.0,
            "Duration should never be negative, even with invalid input"
        );
        assert!(
            cost >= 0,
            "Cost should never be negative, even with invalid input"
        );
    }

    #[test]
    fn test_get_cost_mixed_positive_negative_durations() {
        let endpoint = create_test_endpoint(1000);
        let segments = vec![
            create_test_segment(120.0), // Positive
            create_test_segment(-30.0), // Negative (invalid)
        ];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
    }

    #[test]
    fn test_get_cost_large_values() {
        let endpoint = create_test_endpoint(u64::MAX); // Very large cost
        let segments = vec![create_test_segment(3600.0)]; // 1 hour

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        assert_eq!(duration, 3600.0);
        // Cost should be calculated but not negative
        assert!(cost >= 0);
    }

    #[test]
    fn test_get_cost_fractional_durations() {
        let endpoint = create_test_endpoint(1000);
        let segments = vec![
            create_test_segment(1.5), // 1.5 seconds
            create_test_segment(2.3), // 2.3 seconds
            create_test_segment(0.7), // 0.7 seconds
        ];

        let (duration, cost) = get_cost(&endpoint, &segments);

        assert!(duration >= 0.0, "Duration should never be negative");
        assert!(cost >= 0, "Cost should never be negative");
        assert!((duration - 4.5).abs() < 0.01); // Should sum to 4.5 seconds
    }

    #[test]
    fn test_get_cost_infinity_and_nan_handling() {
        let endpoint = create_test_endpoint(1000);

        // Test with infinity
        let segments = vec![create_test_segment(f32::INFINITY)];
        let (_duration, cost) = get_cost(&endpoint, &segments);
        assert!(
            cost >= 0,
            "Cost should be non-negative even with infinity input"
        );

        // Test with NaN
        let segments = vec![create_test_segment(f32::NAN)];
        let (_duration, cost) = get_cost(&endpoint, &segments);
        assert!(cost >= 0, "Cost should be non-negative even with NaN input");
    }
}
