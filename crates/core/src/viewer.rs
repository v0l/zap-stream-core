use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use uuid::Uuid;
use tokio::task;
use log::debug;

#[derive(Debug, Clone)]
pub struct ViewerInfo {
    pub stream_id: String,
    pub ip_address: String,
    pub user_agent: Option<String>,
    pub last_seen: Instant,
}

#[derive(Debug, Clone)]
pub struct ViewerTracker {
    viewers: Arc<RwLock<HashMap<String, ViewerInfo>>>,
    timeout_duration: Duration,
}

impl ViewerTracker {
    pub fn new() -> Self {
        let tracker = Self {
            viewers: Arc::new(RwLock::new(HashMap::new())),
            timeout_duration: Duration::from_secs(600), // 10 minutes
        };
        
        // Start cleanup task
        let cleanup_tracker = tracker.clone();
        task::spawn(async move {
            cleanup_tracker.cleanup_task().await;
        });
        
        tracker
    }

    pub fn generate_viewer_token() -> String {
        Uuid::new_v4().to_string()
    }

    pub fn track_viewer(&self, token: &str, stream_id: &str, ip_address: &str, user_agent: Option<String>) {
        let mut viewers = self.viewers.write().unwrap();
        
        let viewer_info = ViewerInfo {
            stream_id: stream_id.to_string(),
            ip_address: ip_address.to_string(),
            user_agent,
            last_seen: Instant::now(),
        };
        
        if let Some(existing) = viewers.get(token) {
            debug!("Updating viewer {} for stream {}", token, stream_id);
        } else {
            debug!("New viewer {} for stream {}", token, stream_id);
        }
        
        viewers.insert(token.to_string(), viewer_info);
    }

    pub fn get_viewer_count(&self, stream_id: &str) -> usize {
        let viewers = self.viewers.read().unwrap();
        viewers.values()
            .filter(|v| v.stream_id == stream_id)
            .count()
    }

    pub fn get_active_viewers(&self, stream_id: &str) -> Vec<String> {
        let viewers = self.viewers.read().unwrap();
        viewers.iter()
            .filter(|(_, v)| v.stream_id == stream_id)
            .map(|(token, _)| token.clone())
            .collect()
    }

    pub fn remove_viewer(&self, token: &str) {
        let mut viewers = self.viewers.write().unwrap();
        if let Some(viewer) = viewers.remove(token) {
            debug!("Removed viewer {} from stream {}", token, viewer.stream_id);
        }
    }

    async fn cleanup_task(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60)); // Check every minute
        
        loop {
            interval.tick().await;
            self.cleanup_expired_viewers();
        }
    }

    fn cleanup_expired_viewers(&self) {
        let mut viewers = self.viewers.write().unwrap();
        let now = Instant::now();
        
        let expired_tokens: Vec<String> = viewers.iter()
            .filter(|(_, viewer)| now.duration_since(viewer.last_seen) > self.timeout_duration)
            .map(|(token, _)| token.clone())
            .collect();
        
        for token in expired_tokens {
            if let Some(viewer) = viewers.remove(&token) {
                debug!("Expired viewer {} from stream {} (last seen {:?} ago)", 
                       token, viewer.stream_id, now.duration_since(viewer.last_seen));
            }
        }
    }
}

impl Default for ViewerTracker {
    fn default() -> Self {
        Self::new()
    }
}