use anyhow::{Result, anyhow};
use reqwest::Client;
use super::types::*;

/// HTTP client for Cloudflare Stream API
pub struct CloudflareClient {
    http_client: Client,
    api_token: String,
    account_id: String,
    base_url: String,
}

impl CloudflareClient {
    /// Create a new Cloudflare API client
    pub fn new(api_token: String, account_id: String) -> Self {
        Self {
            http_client: Client::new(),
            api_token,
            account_id,
            base_url: "https://api.cloudflare.com/client/v4".to_string(),
        }
    }

    /// Create a new Live Input
    pub async fn create_live_input(&self, name: &str) -> Result<LiveInputResponse> {
        let url = format!("{}/accounts/{}/stream/live_inputs", 
            self.base_url, self.account_id);
        
        let body = serde_json::json!({
            "meta": {"name": name},
            "recording": {"mode": "automatic"}
        });

        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Cloudflare API error {}: {}", status, error_text));
        }

        Ok(response.json().await?)
    }

    /// Get details of an existing Live Input
    pub async fn get_live_input(&self, uid: &str) -> Result<LiveInputResponse> {
        let url = format!("{}/accounts/{}/stream/live_inputs/{}", 
            self.base_url, self.account_id, uid);

        let response = self.http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Cloudflare API error {}: {}", status, error_text));
        }

        Ok(response.json().await?)
    }

    /// Get Video Assets filtered by Live Input UID
    /// This is the correct way to get HLS URLs - they are in the Video Asset, not the Live Input
    pub async fn get_video_assets(&self, live_input_uid: &str) -> Result<VideoAssetsResponse> {
        let url = format!("{}/accounts/{}/stream?liveInput={}", 
            self.base_url, self.account_id, live_input_uid);

        let response = self.http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Cloudflare API error {}: {}", status, error_text));
        }

        Ok(response.json().await?)
    }

    /// Delete a Live Input
    pub async fn delete_live_input(&self, uid: &str) -> Result<()> {
        let url = format!("{}/accounts/{}/stream/live_inputs/{}", 
            self.base_url, self.account_id, uid);

        let response = self.http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Cloudflare API error {}: {}", status, error_text));
        }

        Ok(())
    }

    /// Setup webhook for Stream Live events
    pub async fn setup_webhook(&self, webhook_url: &str) -> Result<WebhookResponse> {
        let url = format!("{}/accounts/{}/stream/webhook", 
            self.base_url, self.account_id);
        
        let body = serde_json::json!({
            "notificationUrl": webhook_url
        });

        let response = self.http_client
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Cloudflare API error {}: {}", status, error_text));
        }

        Ok(response.json().await?)
    }

    #[cfg(test)]
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn test_create_live_input_success() {
        let mut server = Server::new_async().await;
        let mock = server.mock("POST", "/accounts/test-account/stream/live_inputs")
            .match_header("authorization", "Bearer test-token")
            .match_header("content-type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "success": true,
                "result": {
                    "uid": "test-live-input-uid",
                    "rtmps": {
                        "url": "rtmps://live.cloudflare.com:443/live/",
                        "streamKey": "test-stream-key"
                    },
                    "created": "2025-01-12T00:00:00Z",
                    "status": null
                }
            }"#)
            .create_async()
            .await;

        let client = CloudflareClient::new(
            "test-token".to_string(),
            "test-account".to_string(),
        ).with_base_url(server.url());

        let result = client.create_live_input("test-stream").await;
        assert!(result.is_ok(), "create_live_input should succeed");
        
        let response = result.unwrap();
        assert!(response.success);
        assert_eq!(response.result.uid, "test-live-input-uid");
        assert_eq!(response.result.rtmps.url, "rtmps://live.cloudflare.com:443/live/");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_create_live_input_api_error() {
        let mut server = Server::new_async().await;
        let _mock = server.mock("POST", "/accounts/test-account/stream/live_inputs")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let client = CloudflareClient::new(
            "invalid-token".to_string(),
            "test-account".to_string(),
        ).with_base_url(server.url());

        let result = client.create_live_input("test-stream").await;
        assert!(result.is_err(), "create_live_input should fail with 401");
    }

    #[tokio::test]
    async fn test_get_live_input_success() {
        let mut server = Server::new_async().await;
        let mock = server.mock("GET", "/accounts/test-account/stream/live_inputs/test-uid")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "success": true,
                "result": {
                    "uid": "test-uid",
                    "rtmps": {
                        "url": "rtmps://live.cloudflare.com:443/live/",
                        "streamKey": "test-key"
                    },
                    "created": "2025-01-12T00:00:00Z",
                    "status": "connected"
                }
            }"#)
            .create_async()
            .await;

        let client = CloudflareClient::new(
            "test-token".to_string(),
            "test-account".to_string(),
        ).with_base_url(server.url());

        let result = client.get_live_input("test-uid").await;
        assert!(result.is_ok());
        
        let response = result.unwrap();
        assert_eq!(response.result.status, Some("connected".to_string()));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_get_video_assets_success() {
        let mut server = Server::new_async().await;
        let mock = server.mock("GET", "/accounts/test-account/stream")
            .match_query(mockito::Matcher::UrlEncoded("liveInput".into(), "test-live-input-uid".into()))
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "success": true,
                "result": [{
                    "uid": "video-asset-uid",
                    "playback": {
                        "hls": "https://customer-test.cloudflarestream.com/video-asset-uid/manifest/video.m3u8",
                        "dash": "https://customer-test.cloudflarestream.com/video-asset-uid/manifest/video.mpd"
                    },
                    "liveInput": "test-live-input-uid"
                }]
            }"#)
            .create_async()
            .await;

        let client = CloudflareClient::new(
            "test-token".to_string(),
            "test-account".to_string(),
        ).with_base_url(server.url());

        let result = client.get_video_assets("test-live-input-uid").await;
        assert!(result.is_ok());
        
        let response = result.unwrap();
        assert_eq!(response.result.len(), 1);
        assert_eq!(response.result[0].uid, "video-asset-uid");
        assert!(response.result[0].playback.hls.contains("video.m3u8"));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_delete_live_input_success() {
        let mut server = Server::new_async().await;
        let mock = server.mock("DELETE", "/accounts/test-account/stream/live_inputs/test-uid")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .create_async()
            .await;

        let client = CloudflareClient::new(
            "test-token".to_string(),
            "test-account".to_string(),
        ).with_base_url(server.url());

        let result = client.delete_live_input("test-uid").await;
        assert!(result.is_ok());

        mock.assert_async().await;
    }
}
