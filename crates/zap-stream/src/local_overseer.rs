use crate::endpoint::{get_variants_from_endpoint, parse_capabilities};
use crate::http::{HttpServerPlugin, StreamData};
use crate::settings::{LocalOverseerVariant, OverseerConfig, Settings};
use crate::stream_manager::StreamManager;
use anyhow::{bail, Context};
use async_trait::async_trait;
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper::{Request, Response};
use nostr_sdk::Keys;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use uuid::Uuid;
use zap_stream_core::egress::hls::HlsEgress;
use zap_stream_core::egress::EgressSegment;
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_core::overseer::{IngressInfo, Overseer};
use zap_stream_core::pipeline::{EgressType, PipelineConfig};
use zap_stream_core::variant::StreamMapping;

#[derive(Clone)]
pub struct LocalApi {
    /// Viewer tracker and active stream tracking
    stream_manager: StreamManager,
    /// Relays to publish events to
    relays: Vec<String>,
    /// Nsec to sign nostr events
    nsec: Keys,
    /// Blossom servers
    blossom: Option<Vec<String>>,
    /// Variant config
    variants: Vec<LocalOverseerVariant>,
    /// Public URL for this service
    public_url: String,
}

impl LocalApi {
    pub fn new(
        nsec: String,
        relays: Vec<String>,
        blossom: Option<Vec<String>>,
        variants: Vec<LocalOverseerVariant>,
        public_url: String,
    ) -> Self {
        Self {
            nsec: Keys::parse(&nsec).unwrap(),
            relays,
            blossom,
            variants,
            public_url,
            stream_manager: StreamManager::new(),
        }
    }

    pub fn from_settings(settings: &Settings) -> anyhow::Result<Self> {
        match &settings.overseer {
            OverseerConfig::Local {
                nsec,
                relays,
                blossom,
                variants,
            } => Ok(LocalApi::new(
                nsec.clone(),
                relays.clone(),
                blossom.clone(),
                variants.clone(),
                settings.public_url.clone(),
            )),
            _ => bail!("Invalid overseer config"),
        }
    }
}

impl HttpServerPlugin for LocalApi {
    fn get_active_streams(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<StreamData>>> + Send>> {
        let mgr = self.stream_manager.clone();
        Box::pin(async move {
            let streams = mgr.get_active_stream_ids().await;
            let mut ret = Vec::with_capacity(streams.len());
            for stream_id in streams {
                let viewers = mgr.get_viewer_count(&stream_id).await;
                let url = format!("{}/{}/live.m3u8", &stream_id, HlsEgress::PATH);
                ret.push(StreamData {
                    id: stream_id,
                    title: "".to_string(),
                    summary: None,
                    live_url: url,
                    viewer_count: Some(viewers as _),
                });
            }
            Ok(ret)
        })
    }

    fn track_viewer(
        &self,
        stream_id: &str,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let mgr = self.stream_manager.clone();
        let stream_id = stream_id.to_string();
        let token = token.to_string();
        Box::pin(async move {
            mgr.track_viewer(&stream_id, &token).await;
            Ok(())
        })
    }

    fn handler(self, _request: Request<Incoming>) -> crate::http::HttpFuture {
        // local overseer doesnt implement any of its own endpoints
        Box::pin(async move { Ok(Response::builder().status(404).body(BoxBody::default())?) })
    }
}

#[async_trait]
impl Overseer for LocalApi {
    async fn check_streams(&self) -> anyhow::Result<()> {
        // nothing to do here
        Ok(())
    }

    async fn start_stream(
        &self,
        connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> anyhow::Result<PipelineConfig> {
        let vars = self
            .variants
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<String>>()
            .join(",");
        let caps = parse_capabilities(&Some(vars));

        let cfg = get_variants_from_endpoint(stream_info, &caps)?;

        let egress = vec![EgressType::HLS(
            cfg.variants.iter().map(|v| v.id()).collect(),
        )];

        // TODO: update stream event

        self.stream_manager
            .add_active_stream(&connection.id.to_string())
            .await;

        Ok(PipelineConfig {
            variants: cfg.variants,
            egress,
            ingress_info: stream_info.clone(),
            video_src: cfg
                .video_src
                .map(|s| s.index)
                .context("video stream missing")?,
            audio_src: cfg.audio_src.map(|s| s.index),
        })
    }

    async fn on_segments(
        &self,
        pipeline_id: &Uuid,
        added: &Vec<EgressSegment>,
        deleted: &Vec<EgressSegment>,
    ) -> anyhow::Result<()> {
        // nothing
        Ok(())
    }

    async fn on_thumbnail(
        &self,
        pipeline_id: &Uuid,
        width: usize,
        height: usize,
        path: &PathBuf,
    ) -> anyhow::Result<()> {
        // nothing
        Ok(())
    }

    async fn on_end(&self, pipeline_id: &Uuid) -> anyhow::Result<()> {
        // TODO: update stream event

        self.stream_manager
            .remove_active_stream(&pipeline_id.to_string())
            .await;

        Ok(())
    }

    async fn on_update(&self, pipeline_id: &Uuid) -> anyhow::Result<()> {
        // TODO: update stream event

        Ok(())
    }
}
