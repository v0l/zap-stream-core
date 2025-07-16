use crate::viewer::ViewerTracker;
use chrono::{DateTime, Utc};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
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
    pub fn new() -> Self {
        let (tx, rx) = broadcast::channel(16);
        std::mem::forget(rx); //TODO: clean this

        let r = Self {
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            viewer_tracker: Arc::new(RwLock::new(ViewerTracker::new())),
            stream_viewer_states: Arc::new(RwLock::new(HashMap::new())),
            broadcaster: tx,
        };

        let mgr = r.clone();
        tokio::task::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                {
                    let mut viewers = mgr.viewer_tracker.write().await;
                    viewers.cleanup_expired_viewers();
                }
            }
        });
        r
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
