use crate::viewer::ViewerTracker;
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use log::{info, warn};
use nostr_sdk::serde_json;
use redis::{AsyncCommands, RedisResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use zap_stream_core::ingress::EndpointStats;

#[derive(Clone)]
pub struct StreamViewerState {
    pub last_published_count: usize,
    pub last_update_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveStreamInfo {
    pub stream_id: String,
    pub started_at: DateTime<Utc>,
    pub last_segment_time: Option<DateTime<Utc>>,
    pub node_name: String,

    pub viewers: u32,
    pub average_fps: f32,
    pub target_fps: f32,
    pub frame_count: u64,
    pub endpoint_name: String,
    pub input_resolution: String,
    pub ip_address: String,
    pub ingress_name: String,
    pub endpoint_stats: HashMap<String, EndpointStats>,
}

/// Manages active streams, viewer tracking
#[derive(Clone)]
pub struct StreamManager {
    /// This instances node name
    node_name: String,
    /// Currently active streams with timing info
    /// Any streams which are not contained in this map are dead
    active_streams: Arc<RwLock<HashMap<String, ActiveStreamInfo>>>,
    /// Viewer tracking for active streams
    viewer_tracker: Arc<RwLock<ViewerTracker>>,
    /// Track last published viewer count and update time for each stream
    stream_viewer_states: Arc<RwLock<HashMap<String, StreamViewerState>>>,
    /// Broadcast channel to listen to metrics updates
    broadcaster: broadcast::Sender<ActiveStreamInfo>,
}

impl StreamManager {
    pub fn new(node_name: String) -> Self {
        let (tx, rx) = broadcast::channel(16);
        std::mem::forget(rx); // TODO: fix this

        Self {
            node_name,
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            viewer_tracker: Arc::new(RwLock::new(ViewerTracker::new())),
            stream_viewer_states: Arc::new(RwLock::new(HashMap::new())),
            broadcaster: tx,
        }
    }

    pub fn start_cleanup_task(&self, token: CancellationToken) -> JoinHandle<()> {
        let mgr = self.clone();
        tokio::task::spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = timer.tick() => {
                        let mut viewers = mgr.viewer_tracker.write().await;
                        viewers.cleanup_expired_viewers();
                    }
                }
            }
            info!("Stopping stream manager cleanup");
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
                        let msg: ActiveStreamInfo = match serde_json::from_str(&json) {
                            Ok(msg) => msg,
                            Err(e) => {
                                warn!("Failed to parse active stream info: {} {}", e, json);
                                continue;
                            }
                        };
                        // if stream is not one of ours, track internally to have global view
                        if msg.node_name != node_name {
                            {
                                let mut acc_lock = acc.write().await;
                                acc_lock.insert(msg.stream_id.clone(), msg.clone());
                            }
                            // send to our WS clients
                            if let Err(e) = stats_listener.send(msg) {
                                warn!("Failed to send message: {}", e);
                            }
                        }
                    }
                    Ok(msg) = sub_to_send.recv() => {
                        if msg.node_name == node_name {
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

    pub fn listen_metrics(&self) -> broadcast::Receiver<ActiveStreamInfo> {
        self.broadcaster.subscribe()
    }

    /// Add a new active stream
    pub async fn add_active_stream(
        &self,
        stream_id: &str,
        target_fps: f32,
        endpoint_name: &str,
        input_resolution: &str,
        ingress_name: &str,
        ip: &str,
    ) {
        let now = Utc::now();
        let mut streams = self.active_streams.write().await;
        streams.insert(
            stream_id.to_string(),
            ActiveStreamInfo {
                node_name: self.node_name.clone(),
                stream_id: stream_id.to_string(),
                started_at: now,
                last_segment_time: None,
                average_fps: 0.0,
                viewers: 0,
                target_fps,
                frame_count: 0,
                input_resolution: input_resolution.to_string(),
                endpoint_name: endpoint_name.to_string(),
                ingress_name: ingress_name.to_string(),
                ip_address: ip.to_string(),
                endpoint_stats: HashMap::new(),
            },
        );
    }

    /// Update the last segment time for a stream
    pub async fn update_stream_segment_time(&self, stream_id: &str) {
        let now = Utc::now();
        let mut streams = self.active_streams.write().await;
        if let Some(info) = streams.get_mut(stream_id) {
            info.last_segment_time = Some(now);
        }
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
            let timeout = if let Some(last_segment) = stream_info.last_segment_time {
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
        let viewers = self.viewer_tracker.read().await;
        let viewer_count = viewers.get_viewer_count(stream_id);
        let now = Utc::now();

        let should_update = {
            let viewer_states = self.stream_viewer_states.read().await;
            if let Some(state) = viewer_states.get(stream_id) {
                // Update if count changed OR if 10 minutes have passed since last update
                viewer_count != state.last_published_count
                    || (now - state.last_update_time).num_minutes() >= 10
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
        let viewers = self.viewer_tracker.read().await;
        viewers.get_viewer_count(stream_id)
    }

    pub async fn track_viewer(&self, stream_id: &str, token: &str) {
        let mut viewers = self.viewer_tracker.write().await;
        viewers.track_viewer(stream_id, token);
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
            if let Err(e) = self.broadcaster.send(info.clone()) {
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
            if let Err(e) = self.broadcaster.send(info.clone()) {
                warn!(
                    "Failed to send pipeline metrics to the active stream: {}",
                    e
                );
            }
        }
    }

    pub async fn get_active_streams(&self) -> HashMap<String, ActiveStreamInfo> {
        let streams = self.active_streams.read().await;
        streams.clone()
    }
}
