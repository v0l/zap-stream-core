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

    /// Get the most recent stream ID (UUID) for a user's primary key.
    pub async fn get_latest_stream_id(&self, pubkey_hex: &str) -> Option<String> {
        let upper = pubkey_hex.to_uppercase();
        let row = sqlx::query(
            "SELECT us.id FROM user_stream us \
             JOIN user u ON us.user_id = u.id \
             WHERE HEX(u.pubkey) = ? \
             AND us.stream_key_id IS NULL \
             ORDER BY us.starts DESC LIMIT 1",
        )
        .bind(&upper)
        .fetch_optional(&self.pool)
        .await
        .expect("DB query failed");
        row.map(|r| r.get::<String, _>("id"))
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

    /// `starts` for a stream, as a unix timestamp.
    pub async fn get_stream_starts(&self, stream_id: &str) -> Option<i64> {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "select starts from user_stream where id = ?",
        )
        .bind(stream_id)
        .fetch_optional(&self.pool)
        .await
        .expect("query starts failed")
        .map(|d| d.timestamp())
    }

    /// `title` of a stream.
    pub async fn get_stream_title(&self, stream_id: &str) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>("select title from user_stream where id = ?")
            .bind(stream_id)
            .fetch_optional(&self.pool)
            .await
            .expect("query title failed")
            .flatten()
    }

    /// The Cloudflare recording a stream row currently points at, if any.
    pub async fn get_external_video_id(&self, stream_id: &str) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>(
            "select external_video_id from user_stream where id = ?",
        )
        .bind(stream_id)
        .fetch_optional(&self.pool)
        .await
        .expect("query external_video_id failed")
        .flatten()
    }

    /// Insert an ended stream on the user's account key — no custom key bound — the
    /// shape a finished primary-key broadcast leaves behind. Returns its id.
    pub async fn insert_ended_account_stream(&self, pubkey_hex: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let pubkey_bytes = hex::decode(pubkey_hex).expect("invalid pubkey hex");
        sqlx::query(
            "INSERT INTO user_stream (id, user_id, starts, ends, state) \
             SELECT ?, id, now() - interval 1 hour, now(), 3 FROM user WHERE pubkey = ?",
        )
        .bind(&id)
        .bind(&pubkey_bytes)
        .execute(&self.pool)
        .await
        .expect("Failed to insert ended account stream");
        id
    }
}
