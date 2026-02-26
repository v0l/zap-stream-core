use sqlx::MySqlPool;
use sqlx::Row;

pub struct TestDb {
    pool: MySqlPool,
}

impl TestDb {
    pub async fn connect(connection_string: &str) -> Self {
        let pool = MySqlPool::connect(connection_string)
            .await
            .expect("Failed to connect to test database");
        Self { pool }
    }

    /// Ensure a user row exists for the given hex pubkey.
    pub async fn ensure_user_exists(&self, pubkey_hex: &str) {
        let pubkey_bytes = hex::decode(pubkey_hex).expect("invalid pubkey hex");
        sqlx::query("INSERT IGNORE INTO user (pubkey, balance) VALUES (?, 0)")
            .bind(&pubkey_bytes)
            .execute(&self.pool)
            .await
            .expect("Failed to ensure user exists");
    }

    /// Get the external_id for a user by hex pubkey.
    pub async fn get_external_id(&self, pubkey_hex: &str) -> Option<String> {
        let upper = pubkey_hex.to_uppercase();
        let row = sqlx::query("SELECT external_id FROM user WHERE HEX(pubkey) = ?")
            .bind(&upper)
            .fetch_optional(&self.pool)
            .await
            .expect("DB query failed");
        row.and_then(|r| r.get::<Option<String>, _>("external_id"))
    }

    /// Get the state of a user_stream by UUID string.
    pub async fn get_stream_state(&self, stream_id: &str) -> Option<u8> {
        let row = sqlx::query("SELECT state FROM user_stream WHERE id = ?")
            .bind(stream_id)
            .fetch_optional(&self.pool)
            .await
            .expect("DB query failed");
        row.map(|r| r.get::<u8, _>("state"))
    }

    /// Get the external_id from user_stream_key for a given stream_id.
    pub async fn get_custom_key_external_id(&self, stream_id: &str) -> Option<String> {
        let row =
            sqlx::query("SELECT external_id FROM user_stream_key WHERE stream_id = ? LIMIT 1")
                .bind(stream_id)
                .fetch_optional(&self.pool)
                .await
                .expect("DB query failed");
        row.and_then(|r| r.get::<Option<String>, _>("external_id"))
    }
}
