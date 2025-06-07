use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use uuid::Uuid;
use tokio::task;
use log::debug;
use sha2::{Digest, Sha256};
use bech32::{encode, Hrp};

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

    pub fn generate_viewer_token(ip_address: &str, user_agent: Option<&str>) -> String {
        // Create input string by combining IP address and user agent
        let input = match user_agent {
            Some(ua) => format!("{}{}", ip_address, ua),
            None => ip_address.to_string(),
        };
        
        // Hash the input using SHA-256
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let hash = hasher.finalize();
        
        // Take the first 8 bytes of the hash
        let fingerprint = &hash[..8];
        
        // Bech32 encode with 'vt' (viewer token) as human readable part
        let hrp = Hrp::parse("vt").expect("Valid HRP");
        encode::<bech32::Bech32>(hrp, fingerprint).unwrap_or_else(|_| {
            // Fallback to UUID if bech32 encoding fails
            Uuid::new_v4().to_string()
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_viewer_token_consistency() {
        // Test that the same IP and user agent always generate the same token
        let ip = "192.168.1.1";
        let user_agent = Some("Mozilla/5.0 (Test Browser)");
        
        let token1 = ViewerTracker::generate_viewer_token(ip, user_agent);
        let token2 = ViewerTracker::generate_viewer_token(ip, user_agent);
        
        assert_eq!(token1, token2, "Same IP and user agent should generate identical tokens");
    }

    #[test]
    fn test_generate_viewer_token_different_inputs() {
        // Test that different inputs generate different tokens
        let ip1 = "192.168.1.1";
        let ip2 = "192.168.1.2";
        let user_agent = Some("Mozilla/5.0 (Test Browser)");
        
        let token1 = ViewerTracker::generate_viewer_token(ip1, user_agent);
        let token2 = ViewerTracker::generate_viewer_token(ip2, user_agent);
        
        assert_ne!(token1, token2, "Different IPs should generate different tokens");
    }

    #[test]
    fn test_generate_viewer_token_no_user_agent() {
        // Test that tokens are generated even without user agent
        let ip = "192.168.1.1";
        
        let token1 = ViewerTracker::generate_viewer_token(ip, None);
        let token2 = ViewerTracker::generate_viewer_token(ip, None);
        
        assert_eq!(token1, token2, "Same IP without user agent should generate identical tokens");
    }

    #[test]
    fn test_generate_viewer_token_format() {
        // Test that generated tokens have the expected bech32 format with 'vt' prefix
        let ip = "192.168.1.1";
        let user_agent = Some("Mozilla/5.0 (Test Browser)");
        
        let token = ViewerTracker::generate_viewer_token(ip, user_agent);
        
        assert!(token.starts_with("vt1"), "Token should start with 'vt1' (bech32 with 'vt' HRP)");
        assert!(token.len() > 10, "Token should be reasonably long");
    }

    #[test]
    fn test_generate_viewer_token_different_user_agents() {
        // Test that different user agents generate different tokens
        let ip = "192.168.1.1";
        let user_agent1 = Some("Mozilla/5.0 (Chrome)");
        let user_agent2 = Some("Mozilla/5.0 (Firefox)");
        
        let token1 = ViewerTracker::generate_viewer_token(ip, user_agent1);
        let token2 = ViewerTracker::generate_viewer_token(ip, user_agent2);
        
        assert_ne!(token1, token2, "Different user agents should generate different tokens");
    }
}