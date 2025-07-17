use crate::blossom::{BlobDescriptor, Blossom};
use anyhow::{Result, bail};
use log::{info, warn};
use nostr_sdk::prelude::EventDeletionRequest;
use nostr_sdk::{Client, Event, EventBuilder, EventId, Kind, RelayUrl, Tag, Timestamp};
use std::collections::HashMap;
use std::ops::Add;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use futures::future::join_all;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct N94StreamInfo {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub image: Option<String>,
    pub tags: Vec<String>,
    pub starts: u64,
    pub ends: Option<u64>,
    pub relays: Vec<String>,
    pub variants: Vec<N94Variant>,
    pub goal: Option<String>,
    pub pinned: Option<String>,
}

#[derive(Clone)]
pub struct N94Variant {
    pub id: String,
    pub width: usize,
    pub height: usize,
    pub bitrate: usize,
    pub mime_type: Option<String>,
}

#[derive(Clone)]
pub struct N94Segment {
    pub variant: String,
    pub idx: u64,
    pub duration: f32,
    pub path: PathBuf,
    pub sha256: [u8; 32],
}

#[derive(Clone)]
pub struct N94Publisher {
    /// Nostr client for publishing events
    client: Client,
    /// List of blossom servers to upload segments to
    blossom_servers: Vec<Blossom>,
    /// Published stream event id
    stream_id: Arc<Mutex<Option<EventId>>>,
    /// Track slow/failed servers and disable them
    disabled_servers: Arc<Mutex<HashMap<String, u32>>>,
    /// Maximum number of blossom servers to use concurrently
    max_blossom_servers: usize,
    /// Segment length in seconds (used to calculate timeout)
    segment_length: f32,
}

impl N94Publisher {
    const STREAM_KIND: Kind = Kind::Custom(1053);
    const MAX_FAILURE_COUNT: u32 = 3;

    pub fn new(client: Client, blossom: &Vec<String>, max_blossom_servers: usize, segment_length: f32) -> Self {
        Self {
            client,
            blossom_servers: blossom.iter().map(|s| Blossom::new(s)).collect(),
            stream_id: Arc::new(Mutex::new(None)),
            disabled_servers: Arc::new(Mutex::new(HashMap::new())),
            max_blossom_servers,
            segment_length,
        }
    }

    /// Calculate timeout based on segment length to prevent buffering
    /// Use 80% of segment length as timeout to ensure we don't block too long
    fn calculate_timeout(&self) -> Duration {
        let timeout_secs = ((self.segment_length as f64) * 0.8).max(3.0) as u64;
        Duration::from_secs(timeout_secs)
    }

    /// Converts a blob from blossom into a NIP-94 event (1063)
    fn blob_to_event_builder(&self, blob: &BlobDescriptor) -> Result<EventBuilder> {
        let mut tags = if let Some(tags) = blob.nip94.as_ref() {
            tags.iter().map_while(|v| Tag::parse(v).ok()).collect()
        } else {
            let mut tags = vec![
                Tag::parse(["x", &blob.sha256])?,
                Tag::parse(["url", &blob.url])?,
                Tag::parse(["size", &blob.size.to_string()])?,
            ];
            if let Some(m) = blob.mime_type.as_ref() {
                tags.push(Tag::parse(["m", m])?)
            }
            tags
        };
        tags.push(Tag::parse(["k", "1053"])?);

        Ok(EventBuilder::new(Kind::FileMetadata, "").tags(tags))
    }

    /// Publish stream event
    pub async fn publish_stream(&self, stream: &N94StreamInfo) -> Result<Event> {
        let mut tags = vec![];
        if let Some(t) = &stream.title {
            tags.push(Tag::title(t));
        }
        if let Some(s) = &stream.summary {
            tags.push(Tag::parse(["summary", s])?);
        }
        if let Some(i) = &stream.image {
            tags.push(Tag::parse(["image", i])?);
        }
        if let Some(g) = &stream.goal {
            tags.push(Tag::parse(["goal", g])?);
        }
        if let Some(p) = &stream.pinned {
            tags.push(Tag::parse(["pinned", p])?);
        }
        for t in &stream.tags {
            tags.push(Tag::hashtag(t));
        }
        tags.push(Tag::parse(["starts", stream.starts.to_string().as_str()])?);
        if let Some(e) = &stream.ends {
            tags.push(Tag::parse(["ends", e.to_string().as_str()])?);
        }
        if !stream.relays.is_empty() {
            tags.push(Tag::relays(
                stream.relays.iter().map(|s| RelayUrl::parse(&s).unwrap()),
            ));
        }
        for var in &stream.variants {
            let mut var_tags = vec![
                "variant".to_string(),
                format!("d {}", var.id.to_string()),
                format!("dim {}x{}", var.width, var.height),
                format!("bitrate {}", var.bitrate),
            ];
            if let Some(m) = &var.mime_type {
                var_tags.push(format!("m {}", m));
            }
            tags.push(Tag::parse(var_tags)?);
        }

        let ev = EventBuilder::new(Self::STREAM_KIND, "").tags(tags);
        let ev = self.client.sign_event_builder(ev).await?;
        self.client.send_event(&ev).await?;

        Ok(ev)
    }

    /// Publish a NIP-5E stream event
    pub async fn on_start(&self, stream: N94StreamInfo) -> Result<()> {
        let ev = self.publish_stream(&stream).await?;
        info!("Published N94 stream {}", ev.id.to_hex());
        {
            let mut stream_id = self.stream_id.lock().await;
            stream_id.replace(ev.id.clone());
        }
        Ok(())
    }

    pub async fn on_end(&self) -> Result<()> {
        if let Some(stream_id) = self.stream_id.lock().await.take() {
            let ev = EventBuilder::delete(EventDeletionRequest::new().id(stream_id));
            self.client.send_event_builder(ev).await?;
        }
        Ok(())
    }

    async fn is_server_disabled(&self, server_url: &str) -> bool {
        let disabled_servers = self.disabled_servers.lock().await;
        disabled_servers.get(server_url).copied().unwrap_or(0) >= Self::MAX_FAILURE_COUNT
    }

    async fn mark_server_failure(&self, server_url: &str) {
        let mut disabled_servers = self.disabled_servers.lock().await;
        let failure_count = disabled_servers.entry(server_url.to_string()).or_insert(0);
        *failure_count += 1;
        
        if *failure_count >= Self::MAX_FAILURE_COUNT {
            warn!("Disabling blossom server {} after {} failures", server_url, failure_count);
        }
    }

    async fn mark_server_success(&self, server_url: &str) {
        let mut disabled_servers = self.disabled_servers.lock().await;
        disabled_servers.remove(server_url);
    }

    async fn select_servers_for_upload(&self) -> Vec<&Blossom> {
        use rand::seq::SliceRandom;
        
        let mut available_servers = Vec::new();
        
        for server in &self.blossom_servers {
            let server_url = server.url.to_string();
            if !self.is_server_disabled(&server_url).await {
                available_servers.push(server);
            }
        }
        
        // Shuffle the available servers for random selection
        let mut rng = rand::thread_rng();
        available_servers.shuffle(&mut rng);
        
        // Return up to max_blossom_servers
        available_servers.into_iter().take(self.max_blossom_servers).collect()
    }

    /// Publish segments for the stream
    pub async fn on_new_segment(&self, segments: Vec<N94Segment>) -> Result<()> {
        let stream_event_id = if let Some(stream_id) = *self.stream_id.lock().await {
            stream_id.clone()
        } else {
            bail!("Stream ID not set");
        };

        let mut blobs = vec![];
        let signer = self.client.signer().await?;
        
        for seg in segments {
            let selected_servers = self.select_servers_for_upload().await;
            
            info!("Selected {} out of {} blossom servers for upload", 
                  selected_servers.len(), self.blossom_servers.len());
            
            let timeout = self.calculate_timeout();
            
            // Create upload tasks for parallel execution
            let upload_tasks: Vec<_> = selected_servers.into_iter().map(|b| {
                let server_url = b.url.to_string();
                let seg_path = seg.path.clone();
                let signer = signer.clone();
                let timeout = timeout.clone();
                
                async move {
                    let result = b.upload_with_timeout(&seg_path, &signer, Some("video/mp2t"), timeout).await;
                    (server_url, result)
                }
            }).collect();
            
            // Run all uploads in parallel
            let upload_results = join_all(upload_tasks).await;
            
            // Process results
            for (server_url, result) in upload_results {
                match result {
                    Ok(z) => {
                        blobs.push(z);
                        self.mark_server_success(&server_url).await;
                    }
                    Err(e) => {
                        warn!("Failed to upload segment to {}: {}", server_url, e);
                        if let Some(s) = e.source() {
                            warn!("{}", s);
                        }
                        
                        if e.to_string().contains("timeout") {
                            warn!("Upload timeout detected for server: {}", server_url);
                        }
                        
                        self.mark_server_failure(&server_url).await;
                    }
                }
            }
            if let Some(blob) = blobs.first() {
                let mut n94 = self.blob_to_event_builder(blob)?.tags([
                    Tag::event(stream_event_id),
                    Tag::parse(["d", seg.variant.to_string().as_str()])?,
                    Tag::parse(["index", seg.idx.to_string().as_str()])?,
                    // TODO: use expiration for now to avoid creating events with dead links
                    Tag::expiration(Timestamp::now().add(Duration::from_secs(60))),
                ]);

                // some servers add duration tag
                if !blob
                    .nip94
                    .as_ref()
                    .map(|a| a.iter().any(|b| b[0] == "duration"))
                    .unwrap_or(false)
                {
                    n94 = n94.tag(Tag::parse(["duration", seg.duration.to_string().as_str()])?);
                }

                for b in blobs.iter().skip(1) {
                    n94 = n94.tag(Tag::parse(["url", &b.url])?);
                }
                let n94 = self.client.sign_event_builder(n94).await?;
                let cc = self.client.clone();
                tokio::spawn(async move {
                    if let Err(e) = cc.send_event(&n94).await {
                        warn!("Error sending event: {}", e);
                    }
                });
            }
        }

        Ok(())
    }

    pub async fn on_deleted_segment(&self, segments: Vec<N94Segment>) -> Result<()> {
        let signer = self.client.signer().await?;
        for seg in segments {
            // delete blossom files
            for b in &self.blossom_servers {
                if let Err(e) = b.delete(&seg.sha256, &signer).await {
                    warn!(
                        "Error deleting segment {} on {}: {}",
                        hex::encode(seg.sha256),
                        b.url,
                        e
                    );
                }
            }
            // request deletion from nostr
            // TODO
        }
        Ok(())
    }
}
