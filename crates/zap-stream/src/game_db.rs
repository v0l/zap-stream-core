use crate::settings::TwitchConfig;
use anyhow::{Result, anyhow, bail};
use nostr_sdk::Timestamp;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Clone, Deserialize)]
struct CurrentToken {
    pub access_token: String,
    pub expires_in: u64,
    #[serde(skip)]
    pub loaded: u64,
}

#[derive(Clone)]
pub struct GameDb {
    config: TwitchConfig,
    client: reqwest::Client,
    current_token: Arc<RwLock<Option<CurrentToken>>>,
}

impl GameDb {
    const GAME_FIELDS: &str = "id,cover.image_id,genres.name,name,summary";
    pub fn new(config: TwitchConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            current_token: Default::default(),
        }
    }

    async fn refresh_token(&self) -> Result<CurrentToken> {
        let (should_refresh, tkn) = {
            let read = self.current_token.read().await;
            if let Some(token) = &*read {
                (
                    token.loaded + token.expires_in < Timestamp::now().as_secs(),
                    Some(token.clone()),
                )
            } else {
                (true, None)
            }
        };

        if should_refresh {
            let mut w = self.current_token.write().await;
            let url = format!(
                "https://id.twitch.tv/oauth2/token?client_id={}&client_secret={}&grant_type=client_credentials",
                self.config.client_id, self.config.client_secret
            );

            let rsp = match self
                .client
                .post(&url)
                .header("accept", "application/json")
                .send()
                .await?
                .error_for_status()
            {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to get twitch auth token: {}", e);
                    bail!("Failed to get twitch auth token");
                }
            };
            let mut rsp: CurrentToken = rsp.json().await?;
            rsp.loaded = Timestamp::now().as_secs();
            w.replace(rsp.clone());
            info!("Got new token, expires_in={}", rsp.expires_in);
            Ok(rsp)
        } else {
            Ok(tkn.unwrap())
        }
    }

    async fn post_base(&self, url: impl reqwest::IntoUrl) -> Result<reqwest::RequestBuilder> {
        let token = self.refresh_token().await?;
        Ok(self
            .client
            .post(url)
            .header("client-id", &self.config.client_id)
            .header("authorization", format!("Bearer {}", &token.access_token))
            .header("content-type", "text/plain")
            .header("accept", "application/json"))
    }

    /// Search for games and return the raw JSON string response
    pub async fn search_games(&self, search: &str, limit: u16) -> Result<String> {
        let url = "https://api.igdb.com/v4/games";
        let q = format!(
            "search \"{}\"; fields {}; limit {};",
            Self::GAME_FIELDS,
            search,
            limit
        );

        let rsp = self.post_base(url).await?.body(q).send().await?;
        rsp.text().await.map_err(anyhow::Error::from)
    }

    /// Get a specific game and return the raw JSON string response
    pub async fn get_game(&self, game_id: &str) -> Result<String> {
        let url = "https://api.igdb.com/v4/games";
        let q = format!("fields {}; where id = {};", Self::GAME_FIELDS, game_id);
        let rsp = self.post_base(url).await?.body(q).send().await?;
        rsp.text().await.map_err(anyhow::Error::from)
    }
}
