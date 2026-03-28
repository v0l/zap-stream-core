use std::env;

pub struct TestConfig {
    pub api_port: u16,
    pub db_password: String,
    pub nostr_relay_url: String,
    pub external_container: Option<String>,
    pub db_container: Option<String>,
    pub cloudflare_api_token: Option<String>,
    pub cloudflare_account_id: Option<String>,
}

impl TestConfig {
    pub fn from_env() -> Self {
        Self {
            api_port: env::var("ZS_API_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8080),
            db_password: env::var("DB_ROOT_PASSWORD").unwrap_or_else(|_| "root".to_string()),
            nostr_relay_url: env::var("NOSTR_RELAY_URL")
                .unwrap_or_else(|_| "ws://localhost:3334".to_string()),
            external_container: env::var("ZS_EXTERNAL_CONTAINER").ok(),
            db_container: env::var("ZS_DB_CONTAINER").ok(),
            cloudflare_api_token: env::var("CLOUDFLARE_API_TOKEN").ok(),
            cloudflare_account_id: env::var("CLOUDFLARE_ACCOUNT_ID").ok(),
        }
    }

    pub fn api_base_url(&self) -> String {
        format!("http://localhost:{}/api/v1", self.api_port)
    }

    pub fn db_connection_string(&self) -> String {
        format!(
            "mysql://root:{}@localhost:3306/zap_stream",
            self.db_password
        )
    }
}
