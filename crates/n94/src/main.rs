use anyhow::bail;
use chrono::Utc;
use clap::Parser;
use log::{error, info};
use nostr_sdk::{Client, Filter, Keys, Kind, NostrSigner, TagKind, Url};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zap_stream_core::egress::EgressSegment;
use zap_stream_core::endpoint::{
    EndpointCapability, get_variants_from_endpoint, parse_capabilities,
};
use zap_stream_core::ingress::ConnectionInfo;
use zap_stream_core::listen::try_create_listener;
use zap_stream_core::overseer::{IngressInfo, Overseer, StatsType};
use zap_stream_core::pipeline::{EgressType, PipelineConfig};
use zap_stream_core::variant::{StreamMapping, VariantStream};
use zap_stream_core_nostr::n94::{N94Publisher, N94Segment, N94StreamInfo, N94Variant};

#[derive(Parser, Debug)]
struct Args {
    /// Private key to publish nostr events
    #[clap(short, long)]
    pub nsec: String,

    /// Blossom server to publish to, defaults to users own blossom server list
    #[clap(short, long)]
    pub blossom: Vec<String>,

    /// Maximum number of blossom servers to use concurrently
    #[clap(long, default_value = "3")]
    pub max_blossom_servers: usize,

    /// Segment length in seconds
    #[clap(long, default_value = "6.0")]
    pub segment_length: f32,

    /// Nostr relay to publish events to
    #[clap(
        short,
        long,
        default_values_t = ["wss://relay.damus.io".to_string(),"wss://relay.primal.net".to_string(),"wss://nos.lol".to_string()]
    )]
    pub relay: Vec<String>,

    /// One or more listen endpoints, supported protocols: srt, rtmp, test-pattern
    #[clap(short, long, default_values_t = ["rtmp://localhost:1935".to_string()])]
    pub listen: Vec<String>,

    /// Directory to store temporary files
    #[clap(long)]
    pub data_dir: Option<String>,

    /// Bridge proxy to use when publishing backwards compatible NIP-53 stream event
    #[clap(long)]
    pub nip53_bridge: Option<String>,

    /// Capability configuration
    #[clap(
        long,
        default_values_t = ["variant:1080:6000000".to_string(),"variant:720:4000000".to_string(),"variant:480:2000000".to_string(),"variant:240:1000000".to_string()]
    )]
    pub capability: Vec<String>,

    /// Stream title
    #[clap(short, long)]
    pub title: String,

    /// Long summary line
    #[clap(long)]
    pub summary: Option<String>,

    /// Stream image
    #[clap(long)]
    pub image: Option<String>,

    /// Stream goal
    #[clap(long)]
    pub goal: Option<String>,

    /// Hashtag to add to stream
    #[clap(long)]
    pub hashtag: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init();

    info!("Starting N94 Broadcaster!");
    let mut args = Args::parse();

    // connect nostr relays
    let client = Client::builder().signer(Keys::parse(&args.nsec)?).build();
    for r in &args.relay {
        client.add_relay(r).await?;
    }
    client.connect().await;

    let data_dir = args.data_dir.unwrap_or("./out".to_string());

    let caps = args
        .capability
        .iter()
        .map(|c| parse_capabilities(&Some(c.clone())))
        .flatten()
        .collect();

    // load blossom server list if none specified
    if args.blossom.len() == 0 {
        info!("Loading blossom server list...");
        let pubkey = client.signer().await?.get_public_key().await?;
        let server_list = client
            .fetch_events(
                Filter::new().kind(Kind::Custom(10063)).author(pubkey),
                Duration::from_secs(5),
            )
            .await?;

        if let Some(server_list) = server_list.into_iter().next() {
            let blossom_list: Vec<String> = server_list
                .tags
                .filter(TagKind::Server)
                .map_while(|t| Url::parse(&t.as_slice()[1]).ok())
                .map(|t| t.to_string())
                .collect();
            args.blossom = blossom_list;
        }
    }

    if args.blossom.len() == 0 {
        error!("No blossom servers found, please specify blossom servers manually!");
        return Ok(());
    }
    info!("Nostr relays:");
    for s in &args.relay {
        info!("  - {}", s);
    }
    info!("Blossom servers:");
    for s in &args.blossom {
        info!("  - {}", s);
    }

    let stream_info = N94StreamInfo {
        title: Some(args.title),
        summary: args.summary,
        image: args.image,
        tags: args.hashtag,
        relays: args.relay,
        goal: args.goal,
        ..Default::default()
    };

    // setup overseer
    let overseer: Arc<dyn Overseer> = Arc::new(N94Overseer::new(
        client,
        args.blossom,
        args.max_blossom_servers,
        args.segment_length,
        stream_info,
        caps,
    ));

    // Create ingress listeners
    let mut tasks = vec![];
    for e in args.listen {
        match try_create_listener(&e, &data_dir, &overseer) {
            Ok(l) => tasks.push(l),
            Err(e) => error!("{}", e),
        }
    }

    // Join tasks and get errors
    for handle in tasks {
        if let Err(e) = handle.await? {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}

#[derive(Clone)]
struct N94Overseer {
    pub stream_info: N94StreamInfo,
    pub publisher: N94Publisher,
    pub capabilities: Vec<EndpointCapability>,
    pub segment_length: f32,
}

impl N94Overseer {
    pub fn new(
        client: Client,
        blossom: Vec<String>,
        max_blossom_servers: usize,
        segment_length: f32,
        stream_info: N94StreamInfo,
        capabilities: Vec<EndpointCapability>,
    ) -> Self {
        Self {
            publisher: N94Publisher::new(client, &blossom, max_blossom_servers, segment_length),
            stream_info,
            capabilities,
            segment_length,
        }
    }
}

#[async_trait::async_trait]
impl Overseer for N94Overseer {
    async fn check_streams(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn start_stream(
        &self,
        _connection: &ConnectionInfo,
        stream_info: &IngressInfo,
    ) -> anyhow::Result<PipelineConfig> {
        let cfg = get_variants_from_endpoint(stream_info, &self.capabilities)?;

        if cfg.video_src.is_none() || cfg.variants.is_empty() {
            bail!("No video src found");
        }

        self.publisher
            .on_start(N94StreamInfo {
                starts: Utc::now().timestamp() as _,
                ends: None,
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
                ..self.stream_info.clone()
            })
            .await?;

        Ok(PipelineConfig {
            egress: vec![EgressType::HLS(
                cfg.variants.iter().map(|v| v.id()).collect(),
                self.segment_length,
            )],
            variants: cfg.variants,
            ingress_info: stream_info.clone(),
            video_src: cfg.video_src.unwrap().index,
            audio_src: cfg.audio_src.map(|s| s.index),
        })
    }

    async fn on_segments(
        &self,
        _pipeline_id: &uuid::Uuid,
        added: &Vec<EgressSegment>,
        deleted: &Vec<EgressSegment>,
    ) -> anyhow::Result<()> {
        self.publisher
            .on_new_segment(added.iter().map(|s| into_n94_segment(s)).collect())
            .await?;
        self.publisher
            .on_deleted_segment(deleted.iter().map(|s| into_n94_segment(s)).collect())
            .await?;
        Ok(())
    }

    async fn on_thumbnail(
        &self,
        _pipeline_id: &uuid::Uuid,
        _width: usize,
        _height: usize,
        _path: &PathBuf,
    ) -> anyhow::Result<()> {
        // TODO: upload to blossom?
        Ok(())
    }

    async fn on_end(&self, _pipeline_id: &uuid::Uuid) -> anyhow::Result<()> {
        self.publisher.on_end().await?;
        Ok(())
    }

    async fn on_update(&self, _pipeline_id: &uuid::Uuid) -> anyhow::Result<()> {
        // nothing to do
        Ok(())
    }

    async fn on_stats(&self, _pipeline_id: &uuid::Uuid, stats: StatsType) -> anyhow::Result<()> {
        // nothing to do
        info!("Received stats: {:?}", stats);
        Ok(())
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
