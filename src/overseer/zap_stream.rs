use crate::blossom::{BlobDescriptor, Blossom};
use crate::egress::hls::HlsEgress;
use crate::egress::EgressConfig;
use crate::ingress::ConnectionInfo;
use crate::overseer::{get_default_variants, IngressInfo, Overseer};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::settings::LndSettings;
use crate::variant::StreamMapping;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::Utc;
use fedimint_tonic_lnd::verrpc::VersionRequest;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_MJPEG;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVFrame;
use ffmpeg_rs_raw::Encoder;
use futures_util::FutureExt;
use log::{info, warn};
use nostr_sdk::bitcoin::PrivateKey;
use nostr_sdk::prelude::Coordinate;
use nostr_sdk::{Client, Event, EventBuilder, JsonUtil, Keys, Kind, Tag, ToBech32};
use std::env::temp_dir;
use std::fs::create_dir_all;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;
use uuid::Uuid;
use warp::Filter;
use zap_stream_db::sqlx::Encode;
use zap_stream_db::{UserStream, UserStreamState, ZapStreamDb};

const STREAM_EVENT_KIND: u16 = 30_311;

/// zap.stream NIP-53 overseer
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

        Ok(Self {
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
        })
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

        let kind = Kind::from(STREAM_EVENT_KIND);
        let coord = Coordinate::new(kind, self.keys.public_key).identifier(&stream.id);
        tags.push(Tag::parse(&[
            "alt",
            &format!("Watch live on https://zap.stream/{}", coord.to_bech32()?),
        ])?);
        Ok(EventBuilder::new(kind, "", tags))
    }

    fn blob_to_event_builder(&self, stream: &BlobDescriptor) -> Result<EventBuilder> {
        let tags = if let Some(tags) = stream.nip94.as_ref() {
            tags.iter()
                .map_while(|(k, v)| Tag::parse(&[k, v]).ok())
                .collect()
        } else {
            let mut tags = vec![
                Tag::parse(&["x", &stream.sha256])?,
                Tag::parse(&["url", &stream.url])?,
                Tag::parse(&["size", &stream.size.to_string()])?,
            ];
            if let Some(m) = stream.mime_type.as_ref() {
                tags.push(Tag::parse(&["m", m])?)
            }
            tags
        };

        Ok(EventBuilder::new(Kind::FileMetadata, "", tags))
    }

    async fn publish_stream_event(&self, stream: &UserStream, pubkey: &Vec<u8>) -> Result<Event> {
        let mut extra_tags = vec![
            Tag::parse(&["p", hex::encode(pubkey).as_str(), "", "host"])?,
            Tag::parse(&[
                "streaming",
                self.map_to_public_url(stream, "live.m3u8")?.as_str(),
            ])?,
            Tag::parse(&[
                "image",
                self.map_to_public_url(stream, "thumb.webp")?.as_str(),
            ])?,
        ];
        // flag NIP94 streaming when using blossom servers
        if self.blossom_servers.len() > 0 {
            extra_tags.push(Tag::parse(&["streaming", "nip94"])?);
        }
        let ev = self
            .stream_to_event_builder(stream)?
            .add_tags(extra_tags)
            .sign_with_keys(&self.keys)?;
        self.client.send_event(ev.clone()).await?;
        Ok(ev)
    }

    fn map_to_public_url<'a>(
        &self,
        stream: &UserStream,
        path: impl Into<&'a str>,
    ) -> Result<String> {
        let u: Url = self.public_url.parse()?;
        Ok(u.join(&format!("/{}/", stream.id))?
            .join(path.into())?
            .to_string())
    }
}

#[async_trait]
impl Overseer for ZapStreamOverseer {
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

        let variants = get_default_variants(&stream_info)?;

        let mut egress = vec![];
        egress.push(EgressType::HLS(EgressConfig {
            name: "hls".to_string(),
            variants: variants.iter().map(|v| v.id()).collect(),
        }));

        let user = self.db.get_user(uid).await?;
        let stream_id = Uuid::new_v4();
        // insert new stream record
        let mut new_stream = UserStream {
            id: stream_id.to_string(),
            user_id: uid,
            starts: Utc::now(),
            state: UserStreamState::Live,
            ..Default::default()
        };
        let stream_event = self.publish_stream_event(&new_stream, &user.pubkey).await?;
        new_stream.event = Some(stream_event.as_json());

        self.db.insert_stream(&new_stream).await?;
        self.db.update_stream(&new_stream).await?;
        Ok(PipelineConfig {
            id: stream_id,
            variants,
            egress,
        })
    }

    async fn on_segment(
        &self,
        pipeline_id: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()> {
        // Upload to blossom servers if configured
        let mut blobs = vec![];
        for b in &self.blossom_servers {
            blobs.push(b.upload(path, &self.keys, Some("video/mp2t")).await?);
        }
        if let Some(blob) = blobs.first() {
            let a_tag = format!(
                "{}:{}:{}",
                STREAM_EVENT_KIND,
                self.keys.public_key.to_hex(),
                pipeline_id
            );
            let mut n94 = self.blob_to_event_builder(blob)?.add_tags([
                Tag::parse(&["a", &a_tag])?,
                Tag::parse(&["d", variant_id.to_string().as_str()])?,
                Tag::parse(&["duration", duration.to_string().as_str()])?,
            ]);
            for b in blobs.iter().skip(1) {
                n94 = n94.add_tags(Tag::parse(&["url", &b.url]));
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

        stream.state = UserStreamState::Ended;
        let event = self.publish_stream_event(&stream, &user.pubkey).await?;
        stream.event = Some(event.as_json());
        self.db.update_stream(&stream).await?;

        info!("Stream ended {}", stream.id);
        Ok(())
    }
}
