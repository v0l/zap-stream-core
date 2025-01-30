use crate::{User, UserStream};
use anyhow::Result;
use sqlx::{Executor, MySqlPool, Row};
use uuid::Uuid;

#[derive(Clone)]
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
        if key == "test" {
            // use the 00 pubkey for test sources
            return Ok(Some(self.upsert_user(&[0; 32]).await?));
        }

        Ok(sqlx::query("select id from user where stream_key = ?")
            .bind(key)
            .fetch_optional(&self.db)
            .await?
            .map(|r| r.try_get(0).unwrap()))
    }

    /// Get user by id
    pub async fn get_user(&self, uid: u64) -> Result<User> {
        Ok(sqlx::query_as("select * from user where id = ?")
            .bind(uid)
            .fetch_one(&self.db)
            .await
            .map_err(anyhow::Error::new)?)
    }

    /// Update a users balance
    pub async fn update_user_balance(&self, uid: u64, diff: i64) -> Result<()> {
        sqlx::query("update user set balance = balance + ? where id = ?")
            .bind(diff)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
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

    pub async fn get_stream(&self, id: &Uuid) -> Result<UserStream> {
        Ok(sqlx::query_as("select * from user_stream where id = ?")
            .bind(id.to_string())
            .fetch_one(&self.db)
            .await
            .map_err(anyhow::Error::new)?)
    }

    /// Get the list of active streams
    pub async fn list_live_streams(&self) -> Result<Vec<UserStream>> {
        Ok(sqlx::query_as("select * from user_stream where state = 2")
            .fetch_all(&self.db)
            .await?)
    }

    /// Add [duration] & [cost] to a stream and return the new user balance
    pub async fn tick_stream(
        &self,
        stream_id: &Uuid,
        user_id: u64,
        duration: f32,
        cost: i64,
    ) -> Result<i64> {
        let mut tx = self.db.begin().await?;

        sqlx::query("update user_stream set duration = duration + ?, cost = cost + ? where id = ?")
            .bind(&duration)
            .bind(&cost)
            .bind(stream_id.to_string())
            .execute(&mut *tx)
            .await?;

        sqlx::query("update user set balance = balance - ? where id = ?")
            .bind(&cost)
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;

        let balance: i64 = sqlx::query("select balance from user where id = ?")
            .bind(&user_id)
            .fetch_one(&mut *tx)
            .await?
            .try_get(0)?;

        tx.commit().await?;

        Ok(balance)
    }
}
