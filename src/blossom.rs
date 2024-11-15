use std::collections::HashMap;
use anyhow::Result;
use base64::Engine;
use nostr_sdk::{EventBuilder, JsonUtil, Keys, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::SeekFrom;
use std::ops::Add;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use url::Url;

pub struct Blossom {
    url: Url,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobDescriptor {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub created: u64,
    #[serde(rename = "nip94", skip_serializing_if = "Option::is_none")]
    pub nip94: Option<HashMap<String, String>>,
}

impl Blossom {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.parse().unwrap(),
            client: reqwest::Client::new(),
        }
    }

    async fn hash_file(f: &mut File) -> Result<String> {
        let mut hash = Sha256::new();
        let mut buf: [u8; 1024] = [0; 1024];
        f.seek(SeekFrom::Start(0)).await?;
        while let Ok(data) = f.read(&mut buf).await {
            if data == 0 {
                break;
            }
            hash.update(&buf[..data]);
        }
        let hash = hash.finalize();
        f.seek(SeekFrom::Start(0)).await?;
        Ok(hex::encode(hash))
    }

    pub async fn upload(
        &self,
        from_file: &PathBuf,
        keys: &Keys,
    ) -> Result<BlobDescriptor> {
        let mut f = File::open(from_file).await?;
        let hash = Self::hash_file(&mut f).await?;
        let auth_event = EventBuilder::new(
            Kind::Custom(24242),
            "Upload blob",
            [
                Tag::hashtag("upload"),
                Tag::parse(&["x", &hash])?,
                Tag::expiration(Timestamp::now().add(60)),
            ],
        );

        let auth_event = auth_event.sign_with_keys(keys)?;

        let rsp: BlobDescriptor = self
            .client
            .put(self.url.join("/upload").unwrap())
            .header("Content-Type", "application/octet-stream")
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
            .json()
            .await?;

        Ok(rsp)
    }
}
