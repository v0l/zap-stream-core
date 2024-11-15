use crate::egress::hls::HlsEgress;
use crate::egress::EgressConfig;
use crate::ingress::ConnectionInfo;
use crate::overseer::zap_stream::db::{UserStream, UserStreamState};
use crate::overseer::{get_default_variants, IngressInfo, Overseer};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::settings::LndSettings;
use crate::variant::StreamMapping;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fedimint_tonic_lnd::verrpc::VersionRequest;
use log::info;
use nostr_sdk::bitcoin::PrivateKey;
use nostr_sdk::{JsonUtil, Keys};
use sqlx::{MySqlPool, Row};
use std::env::temp_dir;
use std::fs::create_dir_all;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;

mod db;

/// zap.stream NIP-53 overseer
#[derive(Clone)]
pub struct ZapStreamOverseer {
    db: MySqlPool,
    lnd: fedimint_tonic_lnd::Client,
    client: nostr_sdk::Client,
    keys: Keys,
}

impl ZapStreamOverseer {
    pub async fn new(
        private_key: &str,
        db: &str,
        lnd: &LndSettings,
        relays: &Vec<String>,
    ) -> Result<Self> {
        let db = MySqlPool::connect(db).await?;

        info!("Connected to database, running migrations");
        // automatically run migrations
        sqlx::migrate!().run(&db).await?;

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

    /// Find user by stream key, typical first lookup from ingress
    async fn find_user_stream_key(&self, key: &str) -> Result<Option<u64>> {
        #[cfg(feature = "test-pattern")]
        if key == "test-pattern" {
            // use the 00 pubkey for test sources
            return Ok(Some(self.upsert_user(&[0; 32]).await?));
        }

        Ok(sqlx::query("select id from user where stream_key = ?")
            .bind(key)
            .fetch_optional(&self.db)
            .await?
            .map(|r| r.try_get(0).unwrap()))
    }

    async fn upsert_user(&self, pubkey: &[u8; 32]) -> Result<u64> {
        let res = sqlx::query("insert ignore into user(pubkey) values(?) returning id")
            .bind(pubkey.as_slice())
            .fetch_optional(&self.db)
            .await?;
        match res {
            None => sqlx::query("select id from user where pubkey = ?")
                .bind(pubkey.as_slice())
                .fetch_one(&self.db)
                .await?
                .try_get(0)
                .map_err(anyhow::Error::new),
            Some(res) => res.try_get(0).map_err(anyhow::Error::new),
        }
    }

    async fn create_stream(&self, user_stream: &UserStream) -> Result<u64> {
        sqlx::query(
            "insert into user_stream (user_id, state, starts) values (?, ?, ?) returning id",
        )
        .bind(&user_stream.user_id)
        .bind(&user_stream.state)
        .bind(&user_stream.starts)
        .fetch_one(&self.db)
        .await?
        .try_get(0)
        .map_err(anyhow::Error::new)
    }

    async fn update_stream(&self, user_stream: &UserStream) -> Result<()> {
        sqlx::query(
            "update user_stream set state = ?, starts = ?, ends = ?, title = ?, summary = ?, image = ?, thumb = ?, tags = ?, content_warning = ?, goal = ?, pinned = ?, fee = ?, event = ? where id = ?",
        )
            .bind(&user_stream.state)
            .bind(&user_stream.starts)
            .bind(&user_stream.ends)
            .bind(&user_stream.title)
            .bind(&user_stream.summary)
            .bind(&user_stream.image)
            .bind(&user_stream.thumb)
            .bind(&user_stream.tags)
            .bind(&user_stream.content_warning)
            .bind(&user_stream.goal)
            .bind(&user_stream.pinned)
            .bind(&user_stream.fee)
            .bind(&user_stream.event)
            .bind(&user_stream.id)
            .execute(&self.db)
            .await
            .map_err(anyhow::Error::new)?;
        Ok(())
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
            id: 0,
            user_id: uid,
            starts: Utc::now(),
            state: UserStreamState::Live,
            ..Default::default()
        };

        let stream_id = self.create_stream(&new_stream).await?;
        new_stream.id = stream_id;

        let stream_event = new_stream.publish_stream_event(&self.client).await?;
        new_stream.event = Some(stream_event.as_json());
        self.update_stream(&new_stream).await?;

        Ok(PipelineConfig {
            id: stream_id,
            variants,
            egress,
        })
    }

    async fn on_segment(
        &self,
        pipeline: &Uuid,
        variant_id: &Uuid,
        index: u64,
        duration: f32,
        path: &PathBuf,
    ) -> Result<()> {
        todo!()
    }
}
