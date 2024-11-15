use crate::blossom::{BlobDescriptor, Blossom};
use crate::egress::hls::HlsEgress;
use crate::egress::EgressConfig;
use crate::ingress::ConnectionInfo;
use crate::overseer::{get_default_variants, IngressInfo, Overseer};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::settings::LndSettings;
use crate::variant::StreamMapping;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use fedimint_tonic_lnd::verrpc::VersionRequest;
use futures_util::FutureExt;
use log::info;
use nostr_sdk::bitcoin::PrivateKey;
use nostr_sdk::{Client, Event, EventBuilder, JsonUtil, Keys, Kind, Tag};
use std::env::temp_dir;
use std::fs::create_dir_all;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;
use zap_stream_db::{UserStream, UserStreamState, ZapStreamDb};

const STREAM_EVENT_KIND: u16 = 30_313;

/// zap.stream NIP-53 overseer
pub struct ZapStreamOverseer {
    db: ZapStreamDb,
    lnd: fedimint_tonic_lnd::Client,
    client: Client,
    keys: Keys,
}

impl ZapStreamOverseer {
    pub async fn new(
        private_key: &str,
        db: &str,
        lnd: &LndSettings,
        relays: &Vec<String>,
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
            db,
            lnd,
            client,
            keys,
        })
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

        let out_dir = temp_dir().join("zap-stream");
        create_dir_all(&out_dir)?;

        let variants = get_default_variants(&stream_info)?;

        let mut egress = vec![];
        egress.push(EgressType::HLS(EgressConfig {
            name: "nip94-hls".to_string(),
            out_dir: out_dir.to_string_lossy().to_string(),
            variants: variants.iter().map(|v| v.id()).collect(),
        }));

        // insert new stream record
        let mut new_stream = UserStream {
            id: Uuid::new_v4(),
            user_id: uid,
            starts: Utc::now(),
            state: UserStreamState::Live,
            ..Default::default()
        };
        let stream_event = publish_stream_event(&new_stream, &self.client).await?;
        new_stream.event = Some(stream_event.as_json());

        self.db.insert_stream(&new_stream).await?;
        Ok(PipelineConfig {
            id: new_stream.id,
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
        let blossom = Blossom::new("http://localhost:8881/");
        let blob = blossom.upload(path, &self.keys).await?;

        let a_tag = format!(
            "{}:{}:{}",
            pipeline_id,
            self.keys.public_key.to_hex(),
            STREAM_EVENT_KIND
        );
        // publish nip94 tagged to stream
        let n96 = blob_to_event_builder(&blob)?
            .add_tags(Tag::parse(&["a", &a_tag]))
            .sign_with_keys(&self.keys)?;
        self.client.send_event(n96).await?;
        info!("Published N96 segment for {}", a_tag);

        Ok(())
    }
}

fn stream_to_event_builder(this: &UserStream) -> Result<EventBuilder> {
    let mut tags = vec![
        Tag::parse(&["d".to_string(), this.id.to_string()])?,
        Tag::parse(&["status".to_string(), this.state.to_string()])?,
        Tag::parse(&["starts".to_string(), this.starts.timestamp().to_string()])?,
    ];
    if let Some(ref ends) = this.ends {
        tags.push(Tag::parse(&[
            "ends".to_string(),
            ends.timestamp().to_string(),
        ])?);
    }
    if let Some(ref title) = this.title {
        tags.push(Tag::parse(&["title".to_string(), title.to_string()])?);
    }
    if let Some(ref summary) = this.summary {
        tags.push(Tag::parse(&["summary".to_string(), summary.to_string()])?);
    }
    if let Some(ref image) = this.image {
        tags.push(Tag::parse(&["image".to_string(), image.to_string()])?);
    }
    if let Some(ref thumb) = this.thumb {
        tags.push(Tag::parse(&["thumb".to_string(), thumb.to_string()])?);
    }
    if let Some(ref content_warning) = this.content_warning {
        tags.push(Tag::parse(&[
            "content_warning".to_string(),
            content_warning.to_string(),
        ])?);
    }
    if let Some(ref goal) = this.goal {
        tags.push(Tag::parse(&["goal".to_string(), goal.to_string()])?);
    }
    if let Some(ref pinned) = this.pinned {
        tags.push(Tag::parse(&["pinned".to_string(), pinned.to_string()])?);
    }
    if let Some(ref tags_csv) = this.tags {
        for tag in tags_csv.split(',') {
            tags.push(Tag::parse(&["t".to_string(), tag.to_string()])?);
        }
    }
    Ok(EventBuilder::new(Kind::from(STREAM_EVENT_KIND), "", tags))
}

async fn publish_stream_event(this: &UserStream, client: &Client) -> Result<Event> {
    let ev = stream_to_event_builder(this)?
        .sign(&client.signer().await?)
        .await?;
    client.send_event(ev.clone()).await?;
    Ok(ev)
}

fn blob_to_event_builder(this: &BlobDescriptor) -> Result<EventBuilder> {
    let tags = if let Some(tags) = this.nip94.as_ref() {
        tags.iter()
            .map_while(|(k, v)| Tag::parse(&[k, v]).ok())
            .collect()
    } else {
        let mut tags = vec![
            Tag::parse(&["x", &this.sha256])?,
            Tag::parse(&["url", &this.url])?,
            Tag::parse(&["size", &this.size.to_string()])?,
        ];
        if let Some(m) = this.mime_type.as_ref() {
            tags.push(Tag::parse(&["m", m])?)
        }
        tags
    };

    Ok(EventBuilder::new(Kind::FileMetadata, "", tags))
}
