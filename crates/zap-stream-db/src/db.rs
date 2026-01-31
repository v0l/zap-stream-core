use crate::{
    AuditLog, AuditLogWithPubkeys, IngestEndpoint, Payment, PaymentType, PaymentsSummaryData,
    StreamKeyType, User, UserHistoryEntry, UserPreviousStreams, UserStream, UserStreamForward,
    UserStreamKey,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rand::random;
use sqlx::{MySqlPool, QueryBuilder, Row};
use std::ops::Add;
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
    pub async fn find_user_stream_key(&self, key: &str) -> Result<Option<StreamKeyType>> {
        #[cfg(feature = "test-pattern")]
        if key == "test" {
            // use the 00 pubkey for test sources
            let user_id = self.upsert_user(&[0; 32]).await?;
            return Ok(Some(StreamKeyType::Primary(user_id)));
        }

        // First check primary stream key
        if let Some(user_id) = sqlx::query("select id from user where stream_key = ?")
            .bind(key)
            .fetch_optional(&self.db)
            .await?
            .and_then(|r| r.try_get::<u64, _>(0).ok())
        {
            return Ok(Some(StreamKeyType::Primary(user_id)));
        }

        // Then check temporary stream keys
        if let Some(row) = sqlx::query("select user_id, stream_id from user_stream_key where user_stream_key.key = ? and (expires is null or expires > now())")
            .bind(key)
            .fetch_optional(&self.db)
            .await?
        {
            let user_id: u64 = row.try_get(0)?;
            let stream_id: String = row.try_get(1)?;
            return Ok(Some(StreamKeyType::FixedEventKey { id: user_id, stream_id }));
        }

        Ok(None)
    }

    /// Get user by id
    pub async fn get_user(&self, uid: u64) -> Result<User> {
        Ok(sqlx::query_as("select * from user where id = ?")
            .bind(uid)
            .fetch_one(&self.db)
            .await?)
    }

    /// Get user by id
    pub async fn get_user_by_external_id(&self, external_id: &str) -> Result<Option<User>> {
        Ok(sqlx::query_as("select * from user where external_id = ?")
            .bind(external_id)
            .fetch_optional(&self.db)
            .await?)
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

    /// Mark TOS as accepted for a user
    pub async fn accept_tos(&self, uid: u64) -> Result<()> {
        sqlx::query("update user set tos_accepted = NOW() where id = ?")
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn upsert_user(&self, pubkey: &[u8; 32]) -> Result<u64> {
        let uid = sqlx::query(
            r#"insert into user (pubkey) values(?)
            on duplicate key update
                id = id
            returning id"#,
        )
        .bind(pubkey.as_slice())
        .fetch_one(&self.db)
        .await?
        .try_get(0)?;
        Ok(uid)
    }

    pub async fn insert_stream(&self, user_stream: &UserStream) -> Result<()> {
        sqlx::query(
        "insert into user_stream (id, user_id, state, starts, ends, title, summary, image, thumb, tags, content_warning, goal, pinned, cost, duration, fee, event, endpoint_id, node_name, stream_key_id, external_id)
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
        .bind(&user_stream.id)
        .bind(user_stream.user_id)
        .bind(&user_stream.state)
        .bind(user_stream.starts)
        .bind(user_stream.ends)
        .bind(&user_stream.title)
        .bind(&user_stream.summary)
        .bind(&user_stream.image)
        .bind(&user_stream.thumb)
        .bind(&user_stream.tags)
        .bind(&user_stream.content_warning)
        .bind(&user_stream.goal)
        .bind(&user_stream.pinned)
        .bind(user_stream.cost)
        .bind(user_stream.duration)
        .bind(user_stream.fee)
        .bind(&user_stream.event)
        .bind(user_stream.endpoint_id)
        .bind(&user_stream.node_name)
        .bind(user_stream.stream_key_id)
        .bind(&user_stream.external_id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn update_stream(&self, user_stream: &UserStream) -> Result<()> {
        sqlx::query(
        "update user_stream set state = ?, starts = ?, ends = ?, title = ?, summary = ?, image = ?, thumb = ?, tags = ?, content_warning = ?, goal = ?, pinned = ?, cost = ?, duration = ?, fee = ?, event = ?, endpoint_id = ?, node_name = ?, stream_key_id = ? where id = ?",
    )
        .bind(&user_stream.state)
        .bind(user_stream.starts)
        .bind(user_stream.ends)
        .bind(&user_stream.title)
        .bind(&user_stream.summary)
        .bind(&user_stream.image)
        .bind(&user_stream.thumb)
        .bind(&user_stream.tags)
        .bind(&user_stream.content_warning)
        .bind(&user_stream.goal)
        .bind(&user_stream.pinned)
        .bind(user_stream.cost)
        .bind(user_stream.duration)
        .bind(user_stream.fee)
        .bind(&user_stream.event)
        .bind(user_stream.endpoint_id)
        .bind(&user_stream.node_name)
        .bind(user_stream.stream_key_id)
        .bind(&user_stream.id)
        .execute(&self.db)
        .await
        .map_err(anyhow::Error::new)?;
        Ok(())
    }

    pub async fn get_stream(&self, id: &Uuid) -> Result<UserStream> {
        sqlx::query_as("select * from user_stream where id = ?")
            .bind(id.to_string())
            .fetch_one(&self.db)
            .await
            .map_err(anyhow::Error::new)
    }

    pub async fn try_get_stream(&self, id: &Uuid) -> Result<Option<UserStream>> {
        sqlx::query_as("select * from user_stream where id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.db)
            .await
            .map_err(anyhow::Error::new)
    }

    /// Get the list of active streams
    pub async fn list_live_streams(&self) -> Result<Vec<UserStream>> {
        Ok(sqlx::query_as("select * from user_stream where state = 2")
            .fetch_all(&self.db)
            .await?)
    }

    /// Get the list of active streams for a specific node
    pub async fn list_live_streams_by_node(&self, node_name: &str) -> Result<Vec<UserStream>> {
        Ok(
            sqlx::query_as("select * from user_stream where state = 2 and node_name = ?")
                .bind(node_name)
                .fetch_all(&self.db)
                .await?,
        )
    }

    /// Get recently ended streams by node
    pub async fn list_recently_ended_streams_by_node(
        &self,
        node_name: &str,
    ) -> Result<Vec<UserStream>> {
        Ok(
        sqlx::query_as("select * from user_stream where state = 3 and node_name = ? and ends > now() - interval 10 minute")
            .bind(node_name)
            .fetch_all(&self.db)
            .await?,
    )
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

        if cost > 0 || duration > 0.0 {
            sqlx::query(
                "update user_stream set duration = duration + ?, cost = cost + ? where id = ?",
            )
            .bind(duration)
            .bind(cost)
            .bind(stream_id.to_string())
            .execute(&mut *tx)
            .await?;
        }

        if cost > 0 {
            sqlx::query("update user set balance = balance - ? where id = ?")
                .bind(cost)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }

        let balance: i64 = sqlx::query("select balance from user where id = ?")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?
            .try_get(0)?;

        tx.commit().await?;

        Ok(balance)
    }

    /// Create a new forward
    pub async fn create_forward(
        &self,
        user_id: u64,
        name: &str,
        target: &str,
        external_id: Option<String>,
    ) -> Result<u64> {
        let result =
            sqlx::query("insert into user_stream_forward (user_id, name, target, external_id) values (?, ?, ?, ?)")
                .bind(user_id)
                .bind(name)
                .bind(target)
                .bind(external_id)
                .execute(&self.db)
                .await?;
        Ok(result.last_insert_id())
    }

    /// Get a single user forward
    pub async fn get_user_forward(&self, id: u64) -> Result<Option<UserStreamForward>> {
        Ok(
            sqlx::query_as("select * from user_stream_forward where id = ?")
                .bind(id)
                .fetch_optional(&self.db)
                .await?,
        )
    }

    /// Get all forwards for a user
    pub async fn get_user_forwards(&self, user_id: u64) -> Result<Vec<UserStreamForward>> {
        Ok(
            sqlx::query_as("select * from user_stream_forward where user_id = ?")
                .bind(user_id)
                .fetch_all(&self.db)
                .await?,
        )
    }

    /// Delete a forward
    pub async fn delete_forward(&self, user_id: u64, forward_id: u64) -> Result<()> {
        sqlx::query("delete from user_stream_forward where id = ? and user_id = ?")
            .bind(forward_id)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Update a forward's disabled status
    pub async fn update_forward_disabled(
        &self,
        user_id: u64,
        forward_id: u64,
        disabled: bool,
    ) -> Result<()> {
        sqlx::query("update user_stream_forward set disabled = ? where id = ? and user_id = ?")
            .bind(disabled)
            .bind(forward_id)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Create a new stream key
    pub async fn create_stream_key(
        &self,
        user_id: u64,
        key: &str,
        expires: Option<DateTime<Utc>>,
        stream_id: &str,
    ) -> Result<u64> {
        let result = sqlx::query(
            "insert into user_stream_key (user_id, `key`, expires, stream_id) values (?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(key)
        .bind(expires)
        .bind(stream_id)
        .execute(&self.db)
        .await?;
        Ok(result.last_insert_id())
    }

    /// Get all stream keys for a user
    pub async fn get_user_stream_keys(&self, user_id: u64) -> Result<Vec<UserStreamKey>> {
        Ok(
            sqlx::query_as("select * from user_stream_key where user_id = ?")
                .bind(user_id)
                .fetch_all(&self.db)
                .await?,
        )
    }

    /// Delete a stream key
    pub async fn delete_stream_key(&self, user_id: u64, key_id: u64) -> Result<()> {
        sqlx::query("delete from user_stream_key where id = ? and user_id = ?")
            .bind(key_id)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Create a payment record
    pub async fn create_payment(
        &self,
        payment_hash: &[u8],
        user_id: u64,
        invoice: Option<&str>,
        amount: i64,
        payment_type: PaymentType,
        fee: u64,
        expires: DateTime<Utc>,
        nostr: Option<String>,
        external_data: Option<String>,
    ) -> Result<()> {
        sqlx::query("insert into payment (payment_hash, user_id, invoice, amount, payment_type, fee, nostr, expires, external_data) values (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(payment_hash)
        .bind(user_id)
        .bind(invoice)
        .bind(amount)
        .bind(payment_type)
        .bind(fee)
        .bind(nostr)
        .bind(expires)
        .bind(external_data)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Update payment fee and mark as paid, also update users balance (for deposits/credits)
    pub async fn complete_payment(&self, payment_hash: &[u8], fee: u64) -> Result<bool> {
        let res = sqlx::query("update payment p join user u on p.user_id = u.id set p.fee = ?, p.is_paid = true, u.balance = u.balance + p.amount where p.payment_hash = ? and p.is_paid = false")
        .bind(fee)
        .bind(payment_hash)
        .execute(&self.db)
        .await?;

        // user and payment row updates
        Ok(res.rows_affected() == 2)
    }

    /// Update payment fee and mark as paid for withdrawals (subtracts fee from balance)
    pub async fn complete_withdrawal(&self, payment_hash: &[u8], fee: u64) -> Result<bool> {
        let res = sqlx::query("update payment p join user u on p.user_id = u.id set p.fee = ?, p.is_paid = true, u.balance = u.balance - ? where p.payment_hash = ? and p.is_paid = false")
        .bind(fee)
        .bind(fee)
        .bind(payment_hash)
        .execute(&self.db)
        .await?;

        // user and payment row updates
        Ok(res.rows_affected() == 2)
    }

    /// Get payment by hash
    pub async fn get_payment(&self, payment_hash: &[u8]) -> Result<Option<Payment>> {
        Ok(
            sqlx::query_as("select * from payment where payment_hash = ?")
                .bind(payment_hash)
                .fetch_optional(&self.db)
                .await?,
        )
    }

    /// Get payment history for user
    pub async fn get_payment_history(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<Payment>> {
        Ok(sqlx::query_as(
            "select * from payment where user_id = ? order by created desc limit ? offset ?",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?)
    }

    /// Get pending payments
    pub async fn get_pending_payments(&self) -> Result<Vec<Payment>> {
        Ok(sqlx::query_as(
        "select * from payment where is_paid = false and payment_type in (0,1) and expires > current_timestamp() order by created desc",
    )
        .fetch_all(&self.db)
        .await?)
    }

    /// Get the latest completed payment
    pub async fn get_latest_completed_payment(&self) -> Result<Option<Payment>> {
        Ok(sqlx::query_as(
        "select * from payment where is_paid = true and payment_type in (0,1) order by created desc limit 1",
    )
        .fetch_optional(&self.db)
        .await?)
    }

    /// Get all payments with pagination and filters for admin
    pub async fn get_all_payments(
        &self,
        offset: u64,
        limit: u64,
        user_id: Option<u64>,
        payment_type: Option<PaymentType>,
        is_paid: Option<bool>,
    ) -> Result<Vec<Payment>> {
        let mut query_builder: QueryBuilder<sqlx::MySql> =
            QueryBuilder::new("SELECT * FROM payment");

        let mut has_where = false;
        if let Some(uid) = user_id {
            query_builder.push(" WHERE user_id = ").push_bind(uid);
            has_where = true;
        }
        if let Some(pt) = payment_type {
            if has_where {
                query_builder
                    .push(" AND payment_type = ")
                    .push_bind(pt as u8);
            } else {
                query_builder
                    .push(" WHERE payment_type = ")
                    .push_bind(pt as u8);
                has_where = true;
            }
        }
        if let Some(paid) = is_paid {
            if has_where {
                query_builder.push(" AND is_paid = ").push_bind(paid);
            } else {
                query_builder.push(" WHERE is_paid = ").push_bind(paid);
            }
        }

        query_builder
            .push(" ORDER BY created DESC LIMIT ")
            .push_bind(limit);
        query_builder.push(" OFFSET ").push_bind(offset);

        let payments = query_builder
            .build_query_as::<Payment>()
            .fetch_all(&self.db)
            .await?;

        Ok(payments)
    }

    /// Count all payments with filters for admin
    pub async fn count_all_payments(
        &self,
        user_id: Option<u64>,
        payment_type: Option<PaymentType>,
        is_paid: Option<bool>,
    ) -> Result<u32> {
        let mut query_builder: QueryBuilder<sqlx::MySql> =
            QueryBuilder::new("SELECT COUNT(*) as cnt FROM payment");

        let mut has_where = false;
        if let Some(uid) = user_id {
            query_builder.push(" WHERE user_id = ").push_bind(uid);
            has_where = true;
        }
        if let Some(pt) = payment_type {
            if has_where {
                query_builder
                    .push(" AND payment_type = ")
                    .push_bind(pt as u8);
            } else {
                query_builder
                    .push(" WHERE payment_type = ")
                    .push_bind(pt as u8);
                has_where = true;
            }
        }
        if let Some(paid) = is_paid {
            if has_where {
                query_builder.push(" AND is_paid = ").push_bind(paid);
            } else {
                query_builder.push(" WHERE is_paid = ").push_bind(paid);
            }
        }

        let row = query_builder.build().fetch_one(&self.db).await?;

        Ok(row.try_get::<i64, _>("cnt")? as u32)
    }

    /// Get total stream costs across all users
    pub async fn get_total_stream_costs(&self) -> Result<u64> {
        let row = sqlx::query("select CAST(coalesce(sum(cost), 0) AS SIGNED) as total from user_stream")
            .fetch_one(&self.db)
            .await?;
        Ok(row.try_get::<i64, _>("total")? as u64)
    }

    /// Get total user count
    pub async fn get_total_user_count(&self) -> Result<u32> {
        let row = sqlx::query("select count(*) as cnt from user")
            .fetch_one(&self.db)
            .await?;
        Ok(row.try_get::<i64, _>("cnt")? as u32)
    }

    /// Get total balance across all users
    pub async fn get_total_balance(&self) -> Result<i64> {
        let row = sqlx::query("select CAST(coalesce(sum(balance), 0) AS SIGNED) as total from user")
            .fetch_one(&self.db)
            .await?;
        Ok(row.try_get::<i64, _>("total")?)
    }

    /// Get comprehensive payment summary with all statistics in a single query
    pub async fn get_payments_summary(&self) -> Result<PaymentsSummaryData> {
        Ok(sqlx::query_as(
            "SELECT 
                (SELECT COUNT(*) FROM user) as total_users,
                CAST(COALESCE((SELECT SUM(balance) FROM user), 0) AS SIGNED) as total_balance,
                CAST(COALESCE((SELECT SUM(cost) FROM user_stream), 0) AS SIGNED) as total_stream_costs,
                -- TopUp stats
                (SELECT COUNT(*) FROM payment WHERE payment_type = 0) as topup_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 0), 0) AS SIGNED) as topup_amount,
                (SELECT COUNT(*) FROM payment WHERE payment_type = 0 AND is_paid = true) as topup_paid_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 0 AND is_paid = true), 0) AS SIGNED) as topup_paid_amount,
                -- Zap stats
                (SELECT COUNT(*) FROM payment WHERE payment_type = 1) as zap_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 1), 0) AS SIGNED) as zap_amount,
                (SELECT COUNT(*) FROM payment WHERE payment_type = 1 AND is_paid = true) as zap_paid_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 1 AND is_paid = true), 0) AS SIGNED) as zap_paid_amount,
                -- Credit stats
                (SELECT COUNT(*) FROM payment WHERE payment_type = 2) as credit_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 2), 0) AS SIGNED) as credit_amount,
                (SELECT COUNT(*) FROM payment WHERE payment_type = 2 AND is_paid = true) as credit_paid_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 2 AND is_paid = true), 0) AS SIGNED) as credit_paid_amount,
                -- Withdrawal stats
                (SELECT COUNT(*) FROM payment WHERE payment_type = 3) as withdrawal_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 3), 0) AS SIGNED) as withdrawal_amount,
                (SELECT COUNT(*) FROM payment WHERE payment_type = 3 AND is_paid = true) as withdrawal_paid_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 3 AND is_paid = true), 0) AS SIGNED) as withdrawal_paid_amount,
                -- AdmissionFee stats
                (SELECT COUNT(*) FROM payment WHERE payment_type = 4) as admission_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 4), 0) AS SIGNED) as admission_amount,
                (SELECT COUNT(*) FROM payment WHERE payment_type = 4 AND is_paid = true) as admission_paid_count,
                CAST(COALESCE((SELECT SUM(amount) FROM payment WHERE payment_type = 4 AND is_paid = true), 0) AS SIGNED) as admission_paid_amount"
        )
        .fetch_one(&self.db)
        .await?)
    }

    /// Get payment statistics by type
    pub async fn get_payment_stats_by_type(
        &self,
        payment_type: PaymentType,
    ) -> Result<(u32, i64, u32, i64)> {
        let row = sqlx::query(
            "select count(*) as total_count, 
             CAST(coalesce(sum(amount), 0) AS SIGNED) as total_amount, 
             sum(case when is_paid = true then 1 else 0 end) as paid_count,
             CAST(coalesce(sum(case when is_paid = true then amount else 0 end), 0) AS SIGNED) as paid_amount
             from payment where payment_type = ?",
        )
        .bind(payment_type as u8)
        .fetch_one(&self.db)
        .await?;

        Ok((
            row.try_get::<i64, _>("total_count")? as u32,
            row.try_get::<i64, _>("total_amount")?,
            row.try_get::<i64, _>("paid_count")? as u32,
            row.try_get::<i64, _>("paid_amount")?,
        ))
    }

    /// Update user default stream info
    pub async fn update_user_defaults(
        &self,
        user_id: u64,
        title: Option<&str>,
        summary: Option<&str>,
        image: Option<&str>,
        tags: Option<&str>,
        content_warning: Option<&str>,
        goal: Option<&str>,
    ) -> Result<()> {
        sqlx::query("update user set title = ?, summary = ?, image = ?, tags = ?, content_warning = ?, goal = ? where id = ?")
        .bind(title)
        .bind(summary)
        .bind(image)
        .bind(tags)
        .bind(content_warning)
        .bind(goal)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Get all ingest endpoints
    pub async fn get_ingest_endpoints(&self) -> Result<Vec<IngestEndpoint>> {
        Ok(sqlx::query_as("select * from ingest_endpoint")
            .fetch_all(&self.db)
            .await?)
    }

    /// Get ingest endpoint by id
    pub async fn get_ingest_endpoint(&self, endpoint_id: u64) -> Result<IngestEndpoint> {
        Ok(sqlx::query_as("select * from ingest_endpoint where id = ?")
            .bind(endpoint_id)
            .fetch_one(&self.db)
            .await?)
    }

    /// Create ingest endpoint
    pub async fn create_ingest_endpoint(
        &self,
        name: &str,
        cost: u64,
        capabilities: Option<&str>,
    ) -> Result<u64> {
        let result =
            sqlx::query("insert into ingest_endpoint (name, cost, capabilities) values (?, ?, ?)")
                .bind(name)
                .bind(cost)
                .bind(capabilities)
                .execute(&self.db)
                .await?;
        Ok(result.last_insert_id())
    }

    /// Update ingest endpoint
    pub async fn update_ingest_endpoint(
        &self,
        id: u64,
        name: &str,
        cost: u64,
        capabilities: Option<&str>,
    ) -> Result<()> {
        sqlx::query("update ingest_endpoint set name = ?, cost = ?, capabilities = ? where id = ?")
            .bind(name)
            .bind(cost)
            .bind(capabilities)
            .bind(id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Delete ingest endpoint
    pub async fn delete_ingest_endpoint(&self, id: u64) -> Result<()> {
        sqlx::query("delete from ingest_endpoint where id = ?")
            .bind(id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Check if user is admin
    pub async fn is_admin(&self, uid: u64) -> Result<bool> {
        Ok(sqlx::query("select is_admin from user where id = ?")
            .bind(uid)
            .fetch_one(&self.db)
            .await?
            .try_get(0)?)
    }

    /// Get user id if user is an admin by pubkey
    pub async fn get_admin_uid(&self, pubkey: &[u8; 32]) -> Result<Option<u64>> {
        Ok(
            sqlx::query("select id from user where pubkey = ? and is_admin = true")
                .bind(pubkey.as_slice())
                .fetch_optional(&self.db)
                .await?
                .and_then(|r| r.try_get(0).ok()),
        )
    }

    /// Set user admin status
    pub async fn set_admin(&self, uid: u64, is_admin: bool) -> Result<()> {
        sqlx::query("update user set is_admin = ? where id = ?")
            .bind(is_admin)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Set user blocked status
    pub async fn set_blocked(&self, uid: u64, is_blocked: bool) -> Result<()> {
        sqlx::query("update user set is_blocked = ? where id = ?")
            .bind(is_blocked)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Set user stream dump recording status
    pub async fn set_stream_dump_recording(&self, uid: u64, enabled: bool) -> Result<()> {
        sqlx::query("update user set stream_dump_recording = ? where id = ?")
            .bind(enabled)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Update user's main stream key
    pub async fn update_user_stream_key(&self, uid: u64, new_key: &str) -> Result<()> {
        sqlx::query("update user set stream_key = ? where id = ?")
            .bind(new_key)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Update user's NWC (Nostr Wallet Connect) configuration
    pub async fn update_user_nwc(&self, uid: u64, nwc: Option<&str>) -> Result<()> {
        sqlx::query("update user set nwc = ? where id = ?")
            .bind(nwc)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Get user's NWC (Nostr Wallet Connect) configuration
    pub async fn get_user_nwc(&self, uid: u64) -> Result<Option<String>> {
        Ok(sqlx::query("select nwc from user where id = ?")
            .bind(uid)
            .fetch_optional(&self.db)
            .await?
            .and_then(|row| row.try_get::<Option<String>, _>(0).ok())
            .flatten())
    }

    /// Update users external_id
    pub async fn update_user_external_id(&self, uid: u64, external_id: &str) -> Result<()> {
        sqlx::query("update user set external_id = ? where id = ?")
            .bind(external_id)
            .bind(uid)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Get user by pubkey
    pub async fn get_user_by_pubkey(&self, pubkey: &[u8; 32]) -> Result<Option<User>> {
        Ok(sqlx::query_as("select * from user where pubkey = ?")
            .bind(pubkey.as_slice())
            .fetch_optional(&self.db)
            .await?)
    }

    /// List all users with pagination
    pub async fn list_users(&self, offset: u64, limit: u64) -> Result<(Vec<User>, u64)> {
        let total: i64 = sqlx::query_scalar("select count(*) from user")
            .fetch_one(&self.db)
            .await?;

        let users = sqlx::query_as("select * from user order by created desc limit ? offset ?")
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.db)
            .await?;

        Ok((users, total as u64))
    }

    /// Search users by pubkey prefix (hex encoded)
    pub async fn search_users_by_pubkey(&self, pubkey_prefix: &str) -> Result<(Vec<User>, u64)> {
        let search_pattern = format!("%{}%", pubkey_prefix);

        let total: i64 = sqlx::query_scalar("select count(*) from user where hex(pubkey) like ?")
            .bind(&search_pattern)
            .fetch_one(&self.db)
            .await?;

        let users = sqlx::query_as(
            "select * from user where hex(pubkey) like ? order by created desc limit 50",
        )
        .bind(search_pattern)
        .fetch_all(&self.db)
        .await?;

        Ok((users, total as u64))
    }

    /// Add credit to user balance (admin operation)
    pub async fn add_admin_credit(&self, uid: u64, amount: i64, _memo: Option<&str>) -> Result<()> {
        // Create payment record for admin credit
        let payment_hash: [u8; 32] = random();
        self.create_payment(
            &payment_hash,
            uid,
            None,
            amount,
            PaymentType::Credit,
            0,
            Utc::now().add(chrono::Duration::seconds(1)),
            None,
            None,
        )
        .await?;

        // complete the payment
        self.complete_payment(&payment_hash, 0).await?;

        Ok(())
    }

    /// Get streams for a user with pagination
    pub async fn get_user_streams(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<UserStream>, u64)> {
        let total: i64 = sqlx::query_scalar("select count(*) from user_stream where user_id = ?")
            .bind(user_id)
            .fetch_one(&self.db)
            .await?;

        let streams = sqlx::query_as(
            "select * from user_stream where user_id = ? order by starts desc limit ? offset ?",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok((streams, total as u64))
    }

    /// Get ended streams with costs for a user (for balance history)
    pub async fn get_user_ended_streams(&self, user_id: u64) -> Result<Vec<UserStream>> {
        Ok(sqlx::query_as(
        "select * from user_stream where user_id = ? and state = 3 and cost > 0 order by ends desc",
    )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?)
    }

    /// Get live streams for a user
    pub async fn get_user_live_streams(&self, user_id: u64) -> Result<Vec<UserStream>> {
        Ok(
            sqlx::query_as("select * from user_stream where user_id = ? and state = 2")
                .bind(user_id)
                .fetch_all(&self.db)
                .await?,
        )
    }

    /// Get unified user history combining payments and completed streams with proper pagination
    pub async fn get_unified_user_history(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<UserHistoryEntry>> {
        let query = r#"
            (
                SELECT
                    created,
                    amount,
                    payment_type,
                    nostr,
                    NULL as stream_title,
                    NULL as stream_id
                FROM payment
                WHERE user_id = ? AND is_paid = true
            )
            UNION ALL
            (
                SELECT
                    COALESCE(ends, starts) as created,
                    CAST(cost as INTEGER) as amount,
                    CAST(NULL AS UNSIGNED INTEGER) as payment_type,
                    NULL as nostr,
                    title as stream_title,
                    id as stream_id
                FROM user_stream
                WHERE user_id = ? AND state = 3 AND cost > 0
            )
            ORDER BY created DESC
            LIMIT ? OFFSET ?
        "#;

        Ok(sqlx::query_as(query)
            .bind(user_id)
            .bind(user_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.db)
            .await?)
    }

    /// Log an admin action to the audit table
    pub async fn log_admin_action(
        &self,
        admin_id: u64,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        message: &str,
        metadata: Option<&str>,
    ) -> Result<u64> {
        let result = sqlx::query(
        "insert into audit_log (admin_id, action, target_type, target_id, message, metadata) values (?, ?, ?, ?, ?, ?)",
    )
        .bind(admin_id)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(message)
        .bind(metadata)
        .execute(&self.db)
        .await?;
        Ok(result.last_insert_id())
    }

    /// Get audit logs with pagination
    pub async fn get_audit_logs(&self, offset: u64, limit: u64) -> Result<(Vec<AuditLog>, u64)> {
        let total: i64 = sqlx::query_scalar("select count(*) from audit_log")
            .fetch_one(&self.db)
            .await?;

        let logs = sqlx::query_as("select * from audit_log order by created desc limit ? offset ?")
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.db)
            .await?;

        Ok((logs, total as u64))
    }

    /// Get audit logs with pubkeys included via SQL joins
    pub async fn get_audit_logs_with_pubkeys(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<AuditLogWithPubkeys>, u64)> {
        let total: i64 = sqlx::query_scalar("select count(*) from audit_log")
            .fetch_one(&self.db)
            .await?;

        let logs = sqlx::query_as(
        r#"
            select 
                al.id,
                al.admin_id,
                al.action,
                al.target_type,
                al.target_id,
                al.message,
                al.metadata,
                al.created,
                admin_user.pubkey as admin_pubkey,
                target_user.pubkey as target_pubkey
            from audit_log al
            join user admin_user on al.admin_id = admin_user.id
            left join user target_user on al.target_type = 'user' and al.target_id = cast(target_user.id as char) collate utf8mb4_unicode_ci
            order by al.created desc
            limit ? offset ?
            "#,
    )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok((logs, total as u64))
    }

    /// Get audit logs by admin with pagination
    pub async fn get_audit_logs_by_admin(
        &self,
        admin_id: u64,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<AuditLog>, u64)> {
        let total: i64 = sqlx::query_scalar("select count(*) from audit_log where admin_id = ?")
            .bind(admin_id)
            .fetch_one(&self.db)
            .await?;

        let logs = sqlx::query_as(
            "select * from audit_log where admin_id = ? order by created desc limit ? offset ?",
        )
        .bind(admin_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok((logs, total as u64))
    }

    /// Get audit logs by action with pagination
    pub async fn get_audit_logs_by_action(
        &self,
        action: &str,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<AuditLog>, u64)> {
        let total: i64 = sqlx::query_scalar("select count(*) from audit_log where action = ?")
            .bind(action)
            .fetch_one(&self.db)
            .await?;

        let logs = sqlx::query_as(
            "select * from audit_log where action = ? order by created desc limit ? offset ?",
        )
        .bind(action)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok((logs, total as u64))
    }

    /// Get audit logs by target with pagination
    pub async fn get_audit_logs_by_target(
        &self,
        target_type: &str,
        target_id: &str,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<AuditLog>, u64)> {
        let total: i64 = sqlx::query_scalar(
            "select count(*) from audit_log where target_type = ? and target_id = ?",
        )
        .bind(target_type)
        .bind(target_id)
        .fetch_one(&self.db)
        .await?;

        let logs = sqlx::query_as(
        "select * from audit_log where target_type = ? and target_id = ? order by created desc limit ? offset ?",
    )
        .bind(target_type)
        .bind(target_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?;

        Ok((logs, total as u64))
    }

    /// Get number of live streams (separately counting primary key vs stream key) and last stream ended timestamp for a user
    pub async fn get_user_prev_streams(&self, user_id: u64) -> Result<UserPreviousStreams> {
        sqlx::query_as(
        "select
                cast(coalesce(sum(case when stream_key_id is null then 1 else 0 end), 0) as signed) as live_primary_count,
                cast(coalesce(sum(case when stream_key_id is not null then 1 else 0 end), 0) as signed) as live_stream_key_count,
                (select ends from user_stream where user_id = ? and state = 3 and stream_key_id is null order by ends desc limit 1) as last_ended,
                (select id from user_stream where user_id = ? and state = 3 and stream_key_id is null order by ends desc limit 1) as last_stream_id
             from user_stream
             where user_id = ? and state = 2",
    )
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_one(&self.db)
        .await
        .map_err(anyhow::Error::new)
    }
}
