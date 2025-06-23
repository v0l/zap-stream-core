use data_encoding::BASE32_NOPAD;
use log::debug;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct ViewerInfo {
    pub stream_id: String,
    pub last_seen: Instant,
}

pub struct ViewerTracker {
    viewers: HashMap<String, ViewerInfo>,
    timeout_duration: Duration,
}

impl ViewerTracker {
    pub fn new() -> Self {
        
        Self {
            viewers: HashMap::new(),
            timeout_duration: Duration::from_secs(600), // 10 minutes
        }
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

        // Base32 encode the fingerprint (without padding)
        BASE32_NOPAD.encode(fingerprint).to_lowercase()
    }

    pub fn track_viewer(&mut self, stream_id: &str, token: &str) {
        if let Some(existing) = self.viewers.get_mut(token) {
            debug!("Updating viewer {} for stream {}", token, stream_id);
            existing.last_seen = Instant::now();
        } else {
            debug!("New viewer {} for stream {}", token, stream_id);
            let viewer_info = ViewerInfo {
                stream_id: stream_id.to_string(),
                last_seen: Instant::now(),
            };
            self.viewers.insert(token.to_string(), viewer_info);
        }
    }

    pub fn get_viewer_count(&self, stream_id: &str) -> usize {
        self.viewers
            .values()
            .filter(|v| v.stream_id == stream_id)
            .count()
    }

    pub fn cleanup_expired_viewers(&mut self) {
        let now = Instant::now();

        let expired_tokens: Vec<String> = self
            .viewers
            .iter()
            .filter(|(_, viewer)| now.duration_since(viewer.last_seen) > self.timeout_duration)
            .map(|(token, _)| token.clone())
            .collect();

        for token in expired_tokens {
            if let Some(viewer) = self.viewers.remove(&token) {
                debug!(
                    "Expired viewer {} from stream {} (last seen {:?} ago)",
                    token,
                    viewer.stream_id,
                    now.duration_since(viewer.last_seen)
                );
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

        assert_eq!(
            token1, token2,
            "Same IP and user agent should generate identical tokens"
        );
    }

    #[test]
    fn test_generate_viewer_token_different_inputs() {
        // Test that different inputs generate different tokens
        let ip1 = "192.168.1.1";
        let ip2 = "192.168.1.2";
        let user_agent = Some("Mozilla/5.0 (Test Browser)");

        let token1 = ViewerTracker::generate_viewer_token(ip1, user_agent);
        let token2 = ViewerTracker::generate_viewer_token(ip2, user_agent);

        assert_ne!(
            token1, token2,
            "Different IPs should generate different tokens"
        );
    }

    #[test]
    fn test_generate_viewer_token_no_user_agent() {
        // Test that tokens are generated even without user agent
        let ip = "192.168.1.1";

        let token1 = ViewerTracker::generate_viewer_token(ip, None);
        let token2 = ViewerTracker::generate_viewer_token(ip, None);

        assert_eq!(
            token1, token2,
            "Same IP without user agent should generate identical tokens"
        );
    }

    #[test]
    fn test_generate_viewer_token_format() {
        // Test that generated tokens have the expected base32 format
        let ip = "192.168.1.1";
        let user_agent = Some("Mozilla/5.0 (Test Browser)");

        let token = ViewerTracker::generate_viewer_token(ip, user_agent);

        // Should be base32 encoded (lowercase, no padding)
        assert!(
            token
                .chars()
                .all(|c| "abcdefghijklmnopqrstuvwxyz234567".contains(c)),
            "Token should only contain base32 characters"
        );
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

        assert_ne!(
            token1, token2,
            "Different user agents should generate different tokens"
        );
    }
}
