use crate::GameInfo;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TwitchConfig {
    pub client_id: String,
    pub client_secret: String,
}

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
    // Simple in‑memory cache: key -> (response, timestamp)
    cache: Arc<RwLock<HashMap<String, (Vec<GameInfo>, u64)>>>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl GameDb {
    const GAME_FIELDS: &str = "id,cover.image_id,genres.name,name,summary";
    pub fn new(config: TwitchConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            current_token: Default::default(),
            cache: Default::default(),
        }
    }

    async fn refresh_token(&self) -> Result<CurrentToken> {
        let (should_refresh, tkn) = {
            let read = self.current_token.read().await;
            if let Some(token) = &*read {
                (
                    token.loaded + token.expires_in < now_secs(),
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
            rsp.loaded = now_secs();
            w.replace(rsp.clone());
            info!("Got new token, expires_in={}", rsp.expires_in);
            Ok(rsp)
        } else {
            Ok(tkn.unwrap())
        }
    }

    /// Retrieve a cached response if it exists and is fresh.
    async fn get_cached(&self, key: &str) -> Option<Vec<GameInfo>> {
        let read = self.cache.read().await;
        if let Some((value, ts)) = read.get(key) {
            // Simple TTL of 1 hour (3600 seconds)
            const TTL: u64 = 3600;
            if now_secs() - ts < TTL {
                return Some(value.clone());
            }
        }
        None
    }

    /// Store a response in the cache with the current timestamp.
    async fn set_cached(&self, key: String, value: Vec<GameInfo>) {
        let mut write = self.cache.write().await;
        write.insert(key, (value, now_secs()));
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

    /// Search for games and return the raw JSON string response.
    /// Results are cached for up to one hour to avoid excessive IGDB calls.
    pub async fn search_games(&self, search: &str, limit: u16) -> Result<Vec<GameInfo>> {
        // Create a deterministic cache key.
        let cache_key = format!("search:{}:limit:{}", search, limit);
        if let Some(cached) = self.get_cached(&cache_key).await {
            return Ok(cached);
        }

        let url = "https://api.igdb.com/v4/games";
        let q = format!(
            "search \"{}\"; fields {}; limit {};",
            search,
            Self::GAME_FIELDS,
            limit
        );

        let rsp = self.post_base(url).await?.body(q).send().await?;
        let res: Vec<GameInfo> = rsp.json().await.map_err(anyhow::Error::from)?;
        // Cache the fresh response before returning.
        self.set_cached(cache_key, res.clone()).await;
        Ok(res)
    }

    /// Get a specific game and return the raw JSON string response.
    /// Results are cached for up to one hour.
    pub async fn get_game(&self, game_id: &str) -> Result<GameInfo> {
        let cache_key = format!("game:{}", game_id);
        if let Some(cached) = self.get_cached(&cache_key).await {
            return Ok(cached.into_iter().next().unwrap());
        }
        let url = "https://api.igdb.com/v4/games";
        let q = format!("fields {}; where id = {};", Self::GAME_FIELDS, game_id);
        let rsp = self.post_base(url).await?.body(q).send().await?;
        let res: Vec<GameInfo> = rsp.json().await.map_err(anyhow::Error::from)?;
        self.set_cached(cache_key, res.clone()).await;
        Ok(res.into_iter().next().unwrap())
    }
}
