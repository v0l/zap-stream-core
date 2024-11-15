use crate::UserStream;
use anyhow::Result;
use sqlx::{MySqlPool, Row};

pub struct ZapStreamDb {
    db: MySqlPool,
}

impl ZapStreamDb {
    pub async fn new(db: &str) -> Result<Self> {
        let db = MySqlPool::connect(db).await?;
        Ok(ZapStreamDb { db })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!().run(&self.db).await?;
        Ok(())
    }

    /// Find user by stream key, typical first lookup from ingress
    pub async fn find_user_stream_key(&self, key: &str) -> Result<Option<u64>> {
        #[cfg(feature = "test-pattern")]
        if key == "test-pattern" {
            // use the 00 pubkey for test sources
            return Ok(Some(self.upsert_user(&[0; 32]).await?));
        }

        Ok(sqlx::query("select id from user where stream_key = ?")
            .bind(key)
            .fetch_optional(&self.db)
            .await?
            .map(|r| r.try_get(0).unwrap()))
    }

    pub async fn upsert_user(&self, pubkey: &[u8; 32]) -> Result<u64> {
        let res = sqlx::query("insert ignore into user(pubkey) values(?) returning id")
            .bind(pubkey.as_slice())
            .fetch_optional(&self.db)
            .await?;
        match res {
            None => sqlx::query("select id from user where pubkey = ?")
                .bind(pubkey.as_slice())
                .fetch_one(&self.db)
                .await?
                .try_get(0)
                .map_err(anyhow::Error::new),
            Some(res) => res.try_get(0).map_err(anyhow::Error::new),
        }
    }

    pub async fn insert_stream(&self, user_stream: &UserStream) -> Result<()> {
        sqlx::query("insert into user_stream (id, user_id, state, starts) values (?, ?, ?, ?)")
            .bind(&user_stream.id)
            .bind(&user_stream.user_id)
            .bind(&user_stream.state)
            .bind(&user_stream.starts)
            .execute(&self.db)
            .await?;

        Ok(())
    }

    pub async fn update_stream(&self, user_stream: &UserStream) -> Result<()> {
        sqlx::query(
            "update user_stream set state = ?, starts = ?, ends = ?, title = ?, summary = ?, image = ?, thumb = ?, tags = ?, content_warning = ?, goal = ?, pinned = ?, fee = ?, event = ? where id = ?",
        )
            .bind(&user_stream.state)
            .bind(&user_stream.starts)
            .bind(&user_stream.ends)
            .bind(&user_stream.title)
            .bind(&user_stream.summary)
            .bind(&user_stream.image)
            .bind(&user_stream.thumb)
            .bind(&user_stream.tags)
            .bind(&user_stream.content_warning)
            .bind(&user_stream.goal)
            .bind(&user_stream.pinned)
            .bind(&user_stream.fee)
            .bind(&user_stream.event)
            .bind(&user_stream.id)
            .execute(&self.db)
            .await
            .map_err(anyhow::Error::new)?;
        Ok(())
    }
}
