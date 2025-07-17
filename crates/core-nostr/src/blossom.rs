use crate::hash_file;
use anyhow::Result;
use base64::Engine;
use log::{error, warn};
use nostr_sdk::{EventBuilder, JsonUtil, Kind, NostrSigner, Tag, Timestamp, serde_json};
use serde::{Deserialize, Serialize};
use std::ops::Add;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::File;
use tokio::time::timeout;
use url::Url;

#[derive(Clone)]
pub struct Blossom {
    pub url: Url,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobDescriptor {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(rename = "nip94", skip_serializing_if = "Option::is_none")]
    pub nip94: Option<Vec<Vec<String>>>,
}

impl Blossom {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.parse().unwrap(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn delete(&self, hash: &[u8; 32], signer: &impl NostrSigner) -> Result<()> {
        let id = hex::encode(hash);
        let auth_event = EventBuilder::new(Kind::Custom(24242), "Delete blob").tags([
            Tag::hashtag("delete"),
            Tag::parse(["x", &id])?,
            Tag::expiration(Timestamp::now().add(5)),
        ]);

        let auth_event = auth_event.sign(signer).await?;

        self.client
            .delete(self.url.join(&id).unwrap())
            .header(
                "Authorization",
                &format!(
                    "Nostr {}",
                    base64::engine::general_purpose::STANDARD
                        .encode(auth_event.as_json().as_bytes())
                ),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn upload(
        &self,
        from_file: &PathBuf,
        signer: &impl NostrSigner,
        mime: Option<&str>,
    ) -> Result<BlobDescriptor> {
        self.upload_with_timeout(from_file, signer, mime, Duration::from_secs(30)).await
    }

    pub async fn upload_with_timeout(
        &self,
        from_file: &PathBuf,
        signer: &impl NostrSigner,
        mime: Option<&str>,
        timeout_duration: Duration,
    ) -> Result<BlobDescriptor> {
        let upload_future = async {
            let mut f = File::open(from_file).await?;
            let hash = hex::encode(hash_file(&mut f).await?);
            let auth_event = EventBuilder::new(Kind::Custom(24242), "Upload blob").tags([
                Tag::hashtag("upload"),
                Tag::parse(["x", &hash])?,
                Tag::expiration(Timestamp::now().add(5)),
            ]);

            let auth_event = auth_event.sign(signer).await?;

            let json = self
                .client
                .put(self.url.join("/upload").unwrap())
                .header("Content-Type", mime.unwrap_or("application/octet-stream"))
                .header(
                    "Authorization",
                    &format!(
                        "Nostr {}",
                        base64::engine::general_purpose::STANDARD
                            .encode(auth_event.as_json().as_bytes())
                    ),
                )
                .body(f)
                .send()
                .await?
                .text()
                .await?;

            match serde_json::from_str::<BlobDescriptor>(&json) {
                Ok(blob) => Ok(blob),
                Err(e) => {
                    error!("'{}' {}", json, e);
                    Err(e.into())
                }
            }
        };

        match timeout(timeout_duration, upload_future).await {
            Ok(result) => result,
            Err(_) => {
                warn!("Upload to {} timed out after {:?}", self.url, timeout_duration);
                Err(anyhow::anyhow!("Upload timeout"))
            }
        }
    }
}
