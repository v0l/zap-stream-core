use crate::viewer::ViewerTracker;
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use nostr_sdk::serde_json;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use zap_stream_core::ingress::{ConnectionInfo, EndpointStats};

#[derive(Clone)]
pub struct StreamViewerState {
    pub last_published_count: usize,
    pub last_update_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveStreamInfo {
    pub stream_id: String,
    pub pubkey: String,
    pub user_id: u64,
    pub started_at: DateTime<Utc>,
    pub node_name: String,

    pub last_update: Option<DateTime<Utc>>,
    pub viewers: u32,
    pub average_fps: f32,
    pub target_fps: f32,
    pub frame_count: u64,
    pub endpoint_name: String,
    pub input_resolution: String,
    pub ip_address: String,
    pub ingress_name: String,
    pub endpoint_stats: HashMap<String, EndpointStats>,

    #[serde(skip)]
    pub connection: ConnectionInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_name: String,
    pub cpu: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub uptime: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StreamManagerMetric {
    ActiveStream(ActiveStreamInfo),
    Node(NodeInfo),
}

impl StreamManagerMetric {
    pub fn node_name(&self) -> &String {
        match self {
            StreamManagerMetric::ActiveStream(v) => &v.node_name,
            StreamManagerMetric::Node(v) => &v.node_name,
        }
    }
}

/// Manages active streams, viewer tracking
#[derive(Clone)]
pub struct StreamManager {
    /// Minimum update interval in minutes
    min_update_minutes: i64,
    /// This instances node name
    node_name: String,
    /// Currently active streams with timing info
    /// Any streams which are not contained in this map are dead
    active_streams: Arc<RwLock<HashMap<String, ActiveStreamInfo>>>,
    /// Viewer tracking for active streams
    viewer_tracker: ViewerTracker,
    /// Track last published viewer count and update time for each stream
    stream_viewer_states: Arc<RwLock<HashMap<String, StreamViewerState>>>,
    /// Broadcast channel to listen to metrics updates
    broadcaster: broadcast::Sender<StreamManagerMetric>,
}

impl StreamManager {
    pub fn new(node_name: String) -> Self {
        let (tx, rx) = broadcast::channel(16);
        std::mem::forget(rx); // TODO: fix this

        Self {
            node_name,
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            viewer_tracker: ViewerTracker::new(),
            stream_viewer_states: Arc::new(RwLock::new(HashMap::new())),
            broadcaster: tx,
            min_update_minutes: 5,
        }
    }

    pub async fn new_with_redis(node_name: String, redis: redis::Client) -> Result<Self> {
        let (tx, rx) = broadcast::channel(16);
        std::mem::forget(rx); // TODO: fix this

        Ok(Self {
            node_name,
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            viewer_tracker: ViewerTracker::with_redis(redis).await?,
            stream_viewer_states: Arc::new(RwLock::new(HashMap::new())),
            broadcaster: tx,
            min_update_minutes: 5,
        })
    }

    pub fn start_cleanup_task(&self, token: CancellationToken) -> JoinHandle<()> {
        let mgr = self.clone();
        tokio::task::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = timer.tick() => {
                        mgr.viewer_tracker.cleanup_expired_viewers().await;
                    }
                }
            }
            info!("Stopping stream manager cleanup");
        })
    }

    pub fn start_node_metrics_task(&self, token: CancellationToken) -> JoinHandle<()> {
        let node_name = self.node_name.clone();
        let tx = self.broadcaster.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            let mut sys = sysinfo::System::new();
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = timer.tick() => {
                        sys.refresh_all();

                        let cpu_count = sys.cpus().len();
                        let (memory_total, memory_used) = if let Some(cg) = sys.cgroup_limits() {
                            (cg.total_memory, cg.total_memory - cg.free_memory)
                        } else {
                            (sys.total_memory(), sys.used_memory())
                        };

                        let cpu = if let Some(p) = sys.process(sysinfo::get_current_pid().unwrap()) {
                            p.cpu_usage()
                        } else {
                            sys.global_cpu_usage()
                        } / cpu_count as f32 / 100.0;

                        let info = NodeInfo {
                            node_name: node_name.to_string(),
                            cpu ,
                            memory_used,
                            memory_total,
                            uptime: sysinfo::System::uptime(),
                        };
                        if let Err(e) = tx.send(StreamManagerMetric::Node(info)) {
                            warn!("Failed to send node metrics: {}", e);
                        }
                    }
                }
            }
        })
    }

    pub async fn enable_redis(
        &mut self,
        client: redis::Client,
        token: CancellationToken,
    ) -> Result<JoinHandle<Result<()>>> {
        let mut pub_sub = client.get_async_pubsub().await?;
        let mut pub_conn = client.get_multiplexed_async_connection().await?;
        let acc = self.active_streams.clone();
        let mut sub_to_send = self.listen_metrics();
        let stats_listener = self.broadcaster.clone();
        let node_name = self.node_name.clone();

        const STATS_CHANNEL: &str = "stream-manager-stats";

        // subscribe to stats from other instances
        let h: JoinHandle<Result<()>> = tokio::spawn(async move {
            pub_sub.subscribe(STATS_CHANNEL).await?;
            let mut pub_sub = pub_sub.into_on_message();
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    Some(msg) = pub_sub.next() => {
                        let json: String = match msg.get_payload(){
                            Ok(json) => json,
                            Err(e) => {
                                warn!("Failed to get json data from channel message {}", e);
                                continue;
                            }
                        };
                        let msg: StreamManagerMetric = match serde_json::from_str(&json) {
                            Ok(msg) => msg,
                            Err(e) => {
                                warn!("Failed to parse active stream info: {} {}", e, json);
                                continue;
                            }
                        };
                        // if stream is not one of ours, track internally to have global view
                        if *msg.node_name() != node_name {
                            if let StreamManagerMetric::ActiveStream(active_stream) = &msg {
                                let mut acc_lock = acc.write().await;
                                acc_lock.insert(active_stream.stream_id.clone(), active_stream.clone());
                            }
                            // send to our WS clients
                            if let Err(e) = stats_listener.send(msg) {
                                warn!("Failed to send message: {}", e);
                            }
                        }
                    }
                    Ok(msg) = sub_to_send.recv() => {
                        if *msg.node_name() == node_name {
                            let payload = serde_json::to_string(&msg)?;
                            if let Err(e) = AsyncCommands::publish::<_,_,usize>(&mut pub_conn, STATS_CHANNEL, payload).await {
                                warn!("Failed to publish active stream: {}", e);
                            }
                        }
                    }
                }
            }

            info!("Stopped redis stats publisher.");
            Ok(())
        });

        info!("Redis enabled for stats manager!");
        Ok(h)
    }

    pub fn listen_metrics(&self) -> broadcast::Receiver<StreamManagerMetric> {
        self.broadcaster.subscribe()
    }

    /// Add a new active stream
    pub async fn add_active_stream(
        &self,
        pubkey: &str,
        user_id: u64,
        pipeline_id: &str,
        target_fps: f32,
        input_resolution: &str,
        conn: &ConnectionInfo,
    ) {
        let now = Utc::now();
        let mut streams = self.active_streams.write().await;
        streams.insert(
            pipeline_id.to_string(),
            ActiveStreamInfo {
                pubkey: pubkey.to_string(),
                user_id,
                node_name: self.node_name.clone(),
                stream_id: pipeline_id.to_string(),
                started_at: now,
                last_update: None,
                average_fps: 0.0,
                viewers: 0,
                target_fps,
                frame_count: 0,
                input_resolution: input_resolution.to_string(),
                endpoint_name: conn.app_name.clone(),
                ingress_name: conn.endpoint.clone(),
                ip_address: conn.ip_addr.clone(),
                endpoint_stats: HashMap::new(),
                connection: conn.clone(),
            },
        );
    }

    /// Remove a stream from active tracking
    pub async fn remove_active_stream(&self, stream_id: &str) {
        let mut streams = self.active_streams.write().await;
        streams.remove(stream_id);

        // Clean up viewer tracking state for this stream
        let mut viewer_states = self.stream_viewer_states.write().await;
        let stream_id_str = stream_id.to_string();
        viewer_states.remove(&stream_id_str);
    }

    /// Check if a stream is active and if it should timeout
    pub async fn check_stream_status(&self, stream_id: &str) -> (bool, bool) {
        let now = Utc::now();
        let streams = self.active_streams.read().await;

        if let Some(stream_info) = streams.get(stream_id) {
            // Stream is in active map, but check if it's been inactive too long
            let timeout = if let Some(last_segment) = stream_info.last_update {
                // No segments for 60 seconds = timeout
                (now - last_segment).num_seconds() > 60
            } else {
                // No segments yet, but allow 30 seconds for stream to start producing
                (now - stream_info.started_at).num_seconds() > 30
            };
            (true, timeout)
        } else {
            (false, false)
        }
    }

    /// Check if viewer count should be updated and publish to Nostr if needed
    pub async fn check_and_update_viewer_count(
        &self,
        stream_id: &str,
    ) -> Result<bool, anyhow::Error> {
        let viewer_count = self.viewer_tracker.get_viewer_count(stream_id).await;
        let now = Utc::now();

        let should_update = {
            let viewer_states = self.stream_viewer_states.read().await;
            if let Some(state) = viewer_states.get(stream_id) {
                // Update if count changed OR if 10 minutes have passed since last update
                viewer_count != state.last_published_count
                    || (now - state.last_update_time).num_minutes() >= self.min_update_minutes
            } else {
                // First time tracking this stream, always update
                viewer_count > 0
            }
        };

        if should_update && viewer_count > 0 {
            // Update the tracking state
            let mut viewer_states = self.stream_viewer_states.write().await;
            viewer_states.insert(
                stream_id.to_string(),
                StreamViewerState {
                    last_published_count: viewer_count,
                    last_update_time: now,
                },
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn get_viewer_count(&self, stream_id: &str) -> usize {
        self.viewer_tracker.get_viewer_count(stream_id).await
    }

    pub async fn get_total_viewers(&self) -> u64 {
        let streams = self.active_streams.read().await;
        streams.values().map(|s| s.viewers as u64).sum()
    }

    pub async fn track_viewer(&self, stream_id: &str, token: &str) {
        self.viewer_tracker.track_viewer(stream_id, token).await;
    }

    pub async fn update_pipeline_metrics(
        &self,
        stream_id: &str,
        average_fps: f32,
        frame_count: u64,
    ) {
        let mut streams = self.active_streams.write().await;
        if let Some(info) = streams.get_mut(stream_id) {
            info.average_fps = average_fps;
            info.frame_count = frame_count;
            info.viewers = self.get_viewer_count(stream_id).await as _;
            info.last_update = Some(Utc::now());
            if let Err(e) = self
                .broadcaster
                .send(StreamManagerMetric::ActiveStream(info.clone()))
            {
                warn!(
                    "Failed to send pipeline metrics to the active stream: {}",
                    e
                );
            }
        }
    }

    pub async fn update_endpoint_metrics(&self, stream_id: &str, metrics: EndpointStats) {
        let mut streams = self.active_streams.write().await;
        if let Some(info) = streams.get_mut(stream_id) {
            if let Some(x) = info.endpoint_stats.get_mut(&metrics.name) {
                x.bitrate = metrics.bitrate;
            } else {
                info.endpoint_stats.insert(metrics.name.clone(), metrics);
            }
            if let Err(e) = self
                .broadcaster
                .send(StreamManagerMetric::ActiveStream(info.clone()))
            {
                warn!(
                    "Failed to send pipeline metrics to the active stream: {}",
                    e
                );
            }
        }
    }

    pub async fn get_stream(&self, stream_id: &str) -> Option<ActiveStreamInfo> {
        let streams = self.active_streams.read().await;
        streams.get(stream_id).cloned()
    }

    pub async fn get_active_streams(&self) -> Vec<ActiveStreamInfo> {
        let streams = self.active_streams.read().await;
        streams.values().cloned().collect()
    }
}
