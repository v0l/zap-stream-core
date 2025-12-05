use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::egress::recorder::RecorderEgress;
use zap_stream_core::listen::ListenerEndpoint;
use zap_stream_db::{IngestEndpoint, User};

use crate::streaming_backend::{Endpoint, EndpointCost, StreamingBackend};

/// RML RTMP backend implementation
pub struct RmlRtmpBackend {
    public_url: String,
    endpoints_public_hostname: String,
    listen_endpoints: Vec<String>,
}

impl RmlRtmpBackend {
    pub fn new(public_url: String, endpoints_public_hostname: String, listen_endpoints: Vec<String>) -> Self {
        Self {
            public_url,
            endpoints_public_hostname,
            listen_endpoints,
        }
    }
    
    fn map_to_public_url(&self, path: &str) -> Result<Url> {
        let u: Url = self.public_url.parse()?;
        Ok(u.join(path)?)
    }
}

#[async_trait]
impl StreamingBackend for RmlRtmpBackend {
    async fn generate_stream_key(&self, _pubkey: &[u8; 32]) -> Result<String> {
        Ok(Uuid::new_v4().to_string())
    }
    
    fn is_valid_stream_key(&self, key: &str) -> bool {
        // RML RTMP generates UUIDs: 36 chars with 4 dashes (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
        key.len() == 36 && key.matches('-').count() == 4
    }
    
    async fn get_hls_url(&self, stream_id: &str) -> Result<String> {
        let pipeline_dir = PathBuf::from(stream_id);
        let url = self.map_to_public_url(
            pipeline_dir
                .join(HlsEgress::PATH)
                .join("live.m3u8")
                .to_str()
                .unwrap(),
        )?;
        Ok(url.to_string())
    }
    
    async fn get_recording_url(&self, stream_id: &str) -> Result<Option<String>> {
        let pipeline_dir = PathBuf::from(stream_id);
        let url = self.map_to_public_url(
            pipeline_dir
                .join(RecorderEgress::FILENAME)
                .to_str()
                .unwrap(),
        )?;
        Ok(Some(url.to_string()))
    }
    
    async fn get_thumbnail_url(&self, stream_id: &str) -> Result<String> {
        let pipeline_dir = PathBuf::from(stream_id);
        let url = self.map_to_public_url(
            pipeline_dir
                .join("thumb.webp")
                .to_str()
                .unwrap(),
        )?;
        Ok(url.to_string())
    }
    
    async fn get_viewer_count(&self, _stream_id: &str) -> Result<u32> {
        // For RTMP backend, viewer count is managed by StreamManager
        // This method is not used - overseer directly accesses StreamManager
        Ok(0)
    }
    
    async fn get_ingest_endpoints(&self, user: &User, db_endpoints: &[IngestEndpoint]) -> Result<Vec<Endpoint>> {
        let mut endpoints = Vec::new();
        
        for setting_endpoint in &self.listen_endpoints {
            if let Ok(listener_endpoint) = ListenerEndpoint::from_str(setting_endpoint) {
                for ingest in db_endpoints {
                    if let Some(url) = listener_endpoint
                        .to_public_url(&self.endpoints_public_hostname, &ingest.name)
                    {
                        let protocol = match listener_endpoint {
                            ListenerEndpoint::SRT { .. } => "SRT",
                            ListenerEndpoint::RTMP { .. } => "RTMP",
                            ListenerEndpoint::TCP { .. } => "TCP",
                            _ => continue,
                        };

                        endpoints.push(Endpoint {
                            name: format!("{}-{}", protocol, ingest.name),
                            url,
                            key: user.stream_key.clone(),
                            capabilities: ingest
                                .capabilities
                                .as_ref()
                                .map(|c| c.split(',').map(|s| s.trim().to_string()).collect())
                                .unwrap_or_else(Vec::new),
                            cost: EndpointCost {
                                unit: "min".to_string(),
                                rate: ingest.cost as f32 / 1000.0,
                            },
                        });
                    }
                }
            }
        }
        
        Ok(endpoints)
    }
    
    async fn setup_webhooks(&self, _webhook_url: &str) -> Result<()> {
        // RTMP backend doesn't use webhooks
        Ok(())
    }
    
    fn parse_external_event(&self, _payload: &[u8]) -> Result<Option<crate::streaming_backend::ExternalStreamEvent>> {
        // RTMP backend uses listeners, not webhooks
        Ok(None)
    }
}
