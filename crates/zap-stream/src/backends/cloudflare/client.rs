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
}
