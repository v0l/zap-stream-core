use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use hex::FromHex;
use sqlx::{FromRow, MySqlPool, PgPool, Row};
use std::collections::HashMap;

#[derive(Parser, Debug)]
#[command(name = "legacy-migration-tool")]
#[command(about = "Migrates data from legacy zap-stream system to current system")]
#[command(version = "1.0")]
struct Args {
    /// Legacy PostgreSQL connection string
    #[arg(long = "legacy-connection")]
    legacy_connection: String,

    /// Current MySQL connection string
    #[arg(long = "current-connection")]
    current_connection: String,

    /// Run in dry-run mode (no actual changes)
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Only run validation, don't migrate
    #[arg(long = "validate-only")]
    validate_only: bool,
}

#[derive(Debug, FromRow)]
struct LegacyUser {
    #[sqlx(rename = "PubKey")]
    pub_key: String,
    #[sqlx(rename = "StreamKey")]
    stream_key: String,
    #[sqlx(rename = "Balance")]
    balance: i64,
    #[sqlx(rename = "TosAccepted")]
    tos_accepted: Option<DateTime<Utc>>,
    #[sqlx(rename = "IsAdmin")]
    is_admin: bool,
    #[sqlx(rename = "IsBlocked")]
    is_blocked: bool,
    #[sqlx(rename = "Title")]
    title: Option<String>,
    #[sqlx(rename = "Summary")]
    summary: Option<String>,
    #[sqlx(rename = "Image")]
    image: Option<String>,
    #[sqlx(rename = "Tags")]
    tags: Option<String>,
    #[sqlx(rename = "ContentWarning")]
    content_warning: Option<String>,
    #[sqlx(rename = "Goal")]
    goal: Option<String>,
}

#[derive(Debug, FromRow)]
struct LegacyPayment {
    #[sqlx(rename = "PaymentHash")]
    payment_hash: String,
    #[sqlx(rename = "PubKey")]
    pub_key: String,
    #[sqlx(rename = "Invoice")]
    invoice: Option<String>,
    #[sqlx(rename = "IsPaid")]
    is_paid: bool,
    #[sqlx(rename = "Amount")]
    amount: i64, // Cast from NUMERIC to BIGINT in query
    #[sqlx(rename = "Created")]
    created: DateTime<Utc>,
    #[sqlx(rename = "Nostr")]
    nostr: Option<String>,
    #[sqlx(rename = "Type")]
    payment_type: i32, // Changed from u8 to i32 for PostgreSQL compatibility
    #[sqlx(rename = "Fee")]
    fee: i64, // Cast from NUMERIC to BIGINT in query
}

#[derive(Debug, FromRow)]
struct LegacyUserStream {
    #[sqlx(rename = "Id")]
    id: uuid::Uuid,
    #[sqlx(rename = "PubKey")]
    pub_key: String,
    #[sqlx(rename = "Starts")]
    starts: DateTime<Utc>,
    #[sqlx(rename = "Ends")]
    ends: Option<DateTime<Utc>>,
    #[sqlx(rename = "State")]
    state: i32, // UserStreamState enum as integer
    #[sqlx(rename = "Title")]
    title: Option<String>,
    #[sqlx(rename = "Summary")]
    summary: Option<String>,
    #[sqlx(rename = "Image")]
    image: Option<String>,
    #[sqlx(rename = "Tags")]
    tags: Option<String>,
    #[sqlx(rename = "ContentWarning")]
    content_warning: Option<String>,
    #[sqlx(rename = "Goal")]
    goal: Option<String>,
    #[sqlx(rename = "Event")]
    event: Option<String>,
    #[sqlx(rename = "Thumbnail")]
    thumbnail: Option<String>,
    #[sqlx(rename = "LastSegment")]
    last_segment: Option<DateTime<Utc>>,
    #[sqlx(rename = "MilliSatsCollected")]
    cost: Option<i64>, // decimal in C# -> f64 in Rust, then convert to u64 milisats
    #[sqlx(rename = "Length")]
    duration: Option<f32>, // decimal in C# -> f64 in Rust, then convert to f32 seconds
}

#[derive(Debug, FromRow)]
struct LegacyStreamKey {
    #[sqlx(rename = "Id")]
    id: uuid::Uuid,
    #[sqlx(rename = "UserPubkey")]
    user_pubkey: String,
    #[sqlx(rename = "Key")]
    key: String,
    #[sqlx(rename = "Created")]
    created: DateTime<Utc>,
    #[sqlx(rename = "Expires")]
    expires: Option<DateTime<Utc>>,
    #[sqlx(rename = "StreamId")]
    stream_id: uuid::Uuid,
}

struct MigrationTool {
    legacy_db: PgPool,
    current_db: MySqlPool,
    dry_run: bool,
}

impl MigrationTool {
    async fn new(
        legacy_connection_string: &str,
        current_connection_string: &str,
        dry_run: bool,
    ) -> Result<Self> {
        // Connect to legacy PostgreSQL database
        let legacy_db = PgPool::connect(legacy_connection_string).await?;

        // Connect to current MySQL database
        let current_db = MySqlPool::connect(current_connection_string).await?;

        Ok(MigrationTool {
            legacy_db,
            current_db,
            dry_run,
        })
    }

    async fn migrate_users(&mut self) -> Result<HashMap<String, u64>> {
        println!("🔍 Fetching users from legacy system...");

        let legacy_users = self.fetch_legacy_users().await?;
        println!("📊 Found {} users in legacy system", legacy_users.len());

        let mut pubkey_to_user_id = HashMap::new();

        for (i, legacy_user) in legacy_users.iter().enumerate() {
            println!(
                "👤 Migrating user {}/{}: {}",
                i + 1,
                legacy_users.len(),
                legacy_user.pub_key
            );

            if let Err(e) = self
                .migrate_single_user(legacy_user, &mut pubkey_to_user_id)
                .await
            {
                println!("❌ Failed to migrate user {}: {}", legacy_user.pub_key, e);
                continue;
            }
        }

        println!("✅ User migration completed");
        Ok(pubkey_to_user_id)
    }

    async fn migrate_single_user(
        &self,
        legacy_user: &LegacyUser,
        pubkey_to_user_id: &mut HashMap<String, u64>,
    ) -> Result<()> {
        // Convert hex pubkey to bytes
        let pubkey_bytes =
            <[u8; 32]>::from_hex(&legacy_user.pub_key).context("Invalid pubkey hex format")?;

        if !self.dry_run {
            // Insert/get user in current system using upsert logic
            let user_id = self.upsert_user(&pubkey_bytes, legacy_user).await?;
            pubkey_to_user_id.insert(legacy_user.pub_key.clone(), user_id);

            println!(
                "  ✅ Migrated user {} with ID {}",
                legacy_user.pub_key, user_id
            );
        } else {
            println!("  🔍 [DRY RUN] Would migrate user: {}", legacy_user.pub_key);
            println!("    Balance: {} msat", legacy_user.balance);
            if let Some(tos) = &legacy_user.tos_accepted {
                println!("    TOS accepted: {}", tos);
            }
        }

        Ok(())
    }

    async fn upsert_user(&self, pubkey: &[u8; 32], legacy_user: &LegacyUser) -> Result<u64> {
        // Insert or update user with all properties using ON DUPLICATE KEY UPDATE
        let result = sqlx::query(
            "INSERT INTO user (pubkey, balance, tos_accepted, stream_key, is_admin, is_blocked, 
             title, summary, image, tags, content_warning, goal) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE 
             balance = VALUES(balance),
             tos_accepted = VALUES(tos_accepted),
             stream_key = VALUES(stream_key),
             is_admin = VALUES(is_admin),
             is_blocked = VALUES(is_blocked),
             title = VALUES(title),
             summary = VALUES(summary),
             image = VALUES(image),
             tags = VALUES(tags),
             content_warning = VALUES(content_warning),
             goal = VALUES(goal)",
        )
        .bind(pubkey.as_slice())
        .bind(legacy_user.balance)
        .bind(legacy_user.tos_accepted)
        .bind(&legacy_user.stream_key)
        .bind(legacy_user.is_admin)
        .bind(legacy_user.is_blocked)
        .bind(&legacy_user.title)
        .bind(&legacy_user.summary)
        .bind(&legacy_user.image)
        .bind(&legacy_user.tags)
        .bind(&legacy_user.content_warning)
        .bind(&legacy_user.goal)
        .execute(&self.current_db)
        .await?;

        if result.last_insert_id() > 0 {
            // New user inserted, return the ID
            Ok(result.last_insert_id())
        } else {
            // User already existed and was updated, get their ID
            let row = sqlx::query("SELECT id FROM user WHERE pubkey = ?")
                .bind(pubkey.as_slice())
                .fetch_one(&self.current_db)
                .await?;

            Ok(row.try_get("id")?)
        }
    }

    async fn migrate_payments(&mut self, pubkey_to_user_id: &HashMap<String, u64>) -> Result<()> {
        println!("💰 Fetching payments from legacy system...");

        let legacy_payments = self.fetch_legacy_payments().await?;
        println!(
            "📊 Found {} payments in legacy system",
            legacy_payments.len()
        );

        for (i, legacy_payment) in legacy_payments.iter().enumerate() {
            println!("💳 Migrating payment {}/{}", i + 1, legacy_payments.len());

            if let Err(e) = self
                .migrate_single_payment(legacy_payment, pubkey_to_user_id)
                .await
            {
                println!("❌ Failed to migrate payment: {}", e);
                continue;
            }
        }

        println!("✅ Payment migration completed");
        Ok(())
    }

    async fn migrate_single_payment(
        &self,
        legacy_payment: &LegacyPayment,
        pubkey_to_user_id: &HashMap<String, u64>,
    ) -> Result<()> {
        let user_id = pubkey_to_user_id
            .get(&legacy_payment.pub_key)
            .context("User not found in migration map")?;

        if !self.dry_run {
            // Decode hex string to bytes and ensure it's 32 bytes
            let payment_hash_bytes = hex::decode(&legacy_payment.payment_hash)
                .context("Invalid payment hash hex format")?;
            let payment_hash: [u8; 32] = payment_hash_bytes.try_into().map_err(|v: Vec<u8>| {
                anyhow::anyhow!(
                    "Invalid payment hash length: expected 32 bytes, got {}",
                    v.len()
                )
            })?;
            // Check if payment already exists
            let existing = sqlx::query("SELECT payment_hash FROM payment WHERE payment_hash = ?")
                .bind(payment_hash.as_slice())
                .fetch_optional(&self.current_db)
                .await?;

            if existing.is_none() {
                // Create payment in current system
                sqlx::query(
                    "INSERT INTO payment (payment_hash, user_id, invoice, is_paid, amount, created, nostr, payment_type, fee) 
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
                )
                .bind(payment_hash.as_slice())
                .bind(*user_id)
                .bind(&legacy_payment.invoice)
                .bind(legacy_payment.is_paid)
                .bind(legacy_payment.amount.max(0) as u64) // Convert i64 to u64, ensuring non-negative
                .bind(legacy_payment.created)
                .bind(&legacy_payment.nostr)
                .bind(legacy_payment.payment_type as u8) // Convert i32 to u8
                .bind(legacy_payment.fee.max(0) as u64) // Convert i64 to u64, ensuring non-negative
                .execute(&self.current_db)
                .await?;

                println!("  ✅ Payment migrated: {} msat", legacy_payment.amount);
            } else {
                println!("  ⚠️ Payment already exists, skipping");
            }
        } else {
            println!("  💰 [DRY RUN] Would migrate payment:");
            println!("    Hash: {}", &legacy_payment.payment_hash);
            println!("    Amount: {} msat", legacy_payment.amount);
            println!("    Type: {}", legacy_payment.payment_type);
            println!("    Paid: {}", legacy_payment.is_paid);
        }

        Ok(())
    }

    async fn fetch_legacy_users(&mut self) -> Result<Vec<LegacyUser>> {
        let query = r#"
            SELECT "PubKey", "StreamKey", "Balance", "TosAccepted", "IsAdmin", "IsBlocked", 
                   "Title", "Summary", "Image", "Tags", "ContentWarning", "Goal"
            FROM "Users"
        "#;

        let users = sqlx::query_as::<_, LegacyUser>(query)
            .fetch_all(&self.legacy_db)
            .await?;

        Ok(users)
    }

    async fn fetch_legacy_payments(&mut self) -> Result<Vec<LegacyPayment>> {
        let query = r#"
            SELECT p."PaymentHash", p."PubKey", p."Invoice", p."IsPaid",
                   p."Amount"::bigint as "Amount", p."Created", p."Nostr", p."Type", p."Fee"::bigint as "Fee"
            FROM "Payments" p
            ORDER BY p."Created"
        "#;

        let payments = sqlx::query_as::<_, LegacyPayment>(query)
            .fetch_all(&self.legacy_db)
            .await?;

        Ok(payments)
    }

    async fn migrate_streams(&mut self, pubkey_to_user_id: &HashMap<String, u64>) -> Result<()> {
        println!("🎬 Fetching streams from legacy system...");

        let legacy_streams = self.fetch_legacy_streams().await?;
        println!("📊 Found {} streams in legacy system", legacy_streams.len());

        for (i, legacy_stream) in legacy_streams.iter().enumerate() {
            println!("🎥 Migrating stream {}/{}", i + 1, legacy_streams.len());

            if let Err(e) = self
                .migrate_single_stream(legacy_stream, pubkey_to_user_id)
                .await
            {
                println!("❌ Failed to migrate stream {}: {}", legacy_stream.id, e);
                continue;
            }
        }

        println!("✅ Stream migration completed");
        Ok(())
    }

    async fn migrate_stream_keys(
        &mut self,
        pubkey_to_user_id: &HashMap<String, u64>,
    ) -> Result<()> {
        println!("🔑 Fetching stream keys from legacy system...");

        let legacy_stream_keys = self.fetch_legacy_stream_keys().await?;
        println!(
            "📊 Found {} stream keys in legacy system",
            legacy_stream_keys.len()
        );

        for (i, legacy_stream_key) in legacy_stream_keys.iter().enumerate() {
            println!(
                "🔐 Migrating stream key {}/{}",
                i + 1,
                legacy_stream_keys.len()
            );

            if let Err(e) = self
                .migrate_single_stream_key(legacy_stream_key, pubkey_to_user_id)
                .await
            {
                println!(
                    "❌ Failed to migrate stream key {}: {}",
                    legacy_stream_key.id, e
                );
                continue;
            }
        }

        println!("✅ Stream keys migration completed");
        Ok(())
    }

    async fn migrate_single_stream(
        &self,
        legacy_stream: &LegacyUserStream,
        pubkey_to_user_id: &HashMap<String, u64>,
    ) -> Result<()> {
        let user_id = pubkey_to_user_id
            .get(&legacy_stream.pub_key)
            .context("User not found in migration map")?;

        if !self.dry_run {
            // Check if stream already exists
            let existing = sqlx::query("SELECT id FROM user_stream WHERE id = ?")
                .bind(legacy_stream.id.to_string())
                .fetch_optional(&self.current_db)
                .await?;

            if existing.is_none() {
                // Create stream in current system
                sqlx::query(
                    "INSERT INTO user_stream (id, user_id, starts, ends, state, title, summary, image, thumb, tags, content_warning, goal, cost, duration, event, endpoint_id, last_segment)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                )
                .bind(legacy_stream.id.to_string())
                .bind(*user_id)
                .bind(legacy_stream.starts)
                .bind(legacy_stream.ends)
                .bind(legacy_stream.state as u8) // Convert i32 to u8 for enum
                .bind(&legacy_stream.title)
                .bind(&legacy_stream.summary)
                .bind(&legacy_stream.image)
                .bind(&legacy_stream.thumbnail) // thumbnail -> thumb
                .bind(&legacy_stream.tags)
                .bind(&legacy_stream.content_warning)
                .bind(&legacy_stream.goal)
                .bind(legacy_stream.cost.unwrap_or(0).max(0) as u64) // Convert i64 to u64 milisats, ensure non-negative
                .bind(legacy_stream.duration.unwrap_or(0.0)) // Already f32
                .bind(&legacy_stream.event)
                .bind(None::<u64>) // always unset
                .bind(legacy_stream.last_segment)
                .execute(&self.current_db)
                .await?;

                println!("  ✅ Stream migrated: {}", legacy_stream.id);
            } else {
                println!("  ⚠️ Stream already exists, skipping: {}", legacy_stream.id);
            }
        } else {
            println!("  🎬 [DRY RUN] Would migrate stream:");
            println!("    ID: {}", legacy_stream.id);
            println!(
                "    Title: {}",
                legacy_stream.title.as_deref().unwrap_or("(no title)")
            );
            println!("    State: {}", legacy_stream.state);
            println!("    Starts: {}", legacy_stream.starts);
        }

        Ok(())
    }

    async fn migrate_single_stream_key(
        &self,
        legacy_stream_key: &LegacyStreamKey,
        pubkey_to_user_id: &HashMap<String, u64>,
    ) -> Result<()> {
        let user_id = pubkey_to_user_id
            .get(&legacy_stream_key.user_pubkey)
            .context("User not found in migration map")?;

        if !self.dry_run {
            // Check if stream key already exists
            let existing = sqlx::query("SELECT id FROM user_stream_key WHERE `key` = ?")
                .bind(&legacy_stream_key.key)
                .fetch_optional(&self.current_db)
                .await?;

            if existing.is_none() {
                // Create stream key in current system
                sqlx::query(
                    "INSERT INTO user_stream_key (user_id, `key`, created, expires, stream_id)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(*user_id)
                .bind(&legacy_stream_key.key)
                .bind(legacy_stream_key.created)
                .bind(legacy_stream_key.expires)
                .bind(legacy_stream_key.stream_id.to_string())
                .execute(&self.current_db)
                .await?;

                println!(
                    "  ✅ Stream key migrated for user: {}",
                    legacy_stream_key.user_pubkey
                );
            } else {
                println!("  ⚠️ Stream key already exists, skipping");
            }
        } else {
            println!("  🔐 [DRY RUN] Would migrate stream key:");
            println!("    User: {}", legacy_stream_key.user_pubkey);
            println!("    Key: {}", legacy_stream_key.key);
            println!("    Stream ID: {}", legacy_stream_key.stream_id);
            if let Some(expires) = &legacy_stream_key.expires {
                println!("    Expires: {}", expires);
            } else {
                println!("    Expires: Never");
            }
        }

        Ok(())
    }

    async fn fetch_legacy_streams(&mut self) -> Result<Vec<LegacyUserStream>> {
        let query = r#"
            SELECT s."Id", s."PubKey", s."Starts", s."Ends", s."State", s."Title", 
                   s."Summary", s."Image", s."Tags", s."ContentWarning", s."Goal",
                   s."Event", s."Thumbnail", s."LastSegment",
                   s."MilliSatsCollected"::bigint as "MilliSatsCollected", s."Length"::real as "Length"
            FROM "Streams" s
            ORDER BY s."Starts"
        "#;

        let streams = sqlx::query_as::<_, LegacyUserStream>(query)
            .fetch_all(&self.legacy_db)
            .await?;

        Ok(streams)
    }

    async fn fetch_legacy_stream_keys(&mut self) -> Result<Vec<LegacyStreamKey>> {
        let query = r#"
            SELECT sk."Id", sk."UserPubkey", sk."Key", sk."Created", sk."Expires", sk."StreamId"
            FROM "StreamKeys" sk
            ORDER BY sk."Created"
        "#;

        let stream_keys = sqlx::query_as::<_, LegacyStreamKey>(query)
            .fetch_all(&self.legacy_db)
            .await?;

        Ok(stream_keys)
    }

    async fn validate_migration(&mut self, pubkey_to_user_id: &HashMap<String, u64>) -> Result<()> {
        println!("🔍 Validating migration...");

        // Validate user count
        let legacy_user_count = self.get_legacy_user_count().await?;
        let current_user_count = self.get_current_user_count().await?;

        println!(
            "📊 User counts - Legacy: {}, Current: {}",
            legacy_user_count, current_user_count
        );

        // Validate payment count
        let legacy_payment_count = self.get_legacy_payment_count().await?;
        let current_payment_count = self.get_current_payment_count().await?;

        println!(
            "📊 Payment counts - Legacy: {}, Current: {}",
            legacy_payment_count, current_payment_count
        );

        // Validate stream count
        let legacy_stream_count = self.get_legacy_stream_count().await?;
        let current_stream_count = self.get_current_stream_count().await?;

        println!(
            "📊 Stream counts - Legacy: {}, Current: {}",
            legacy_stream_count, current_stream_count
        );

        // Validate stream key count
        let legacy_stream_key_count = self.get_legacy_stream_key_count().await?;
        let current_stream_key_count = self.get_current_stream_key_count().await?;

        println!(
            "📊 Stream key counts - Legacy: {}, Current: {}",
            legacy_stream_key_count, current_stream_key_count
        );

        // Validate balance consistency for a sample of users
        println!("💰 Validating balance consistency...");
        for (pubkey, user_id) in pubkey_to_user_id.iter() {
            if let Err(e) = self.validate_user_balance(pubkey, *user_id).await {
                println!("⚠️ Balance validation failed for user {}: {}", pubkey, e);
            }
        }

        println!("✅ Validation completed");
        Ok(())
    }

    async fn get_legacy_user_count(&mut self) -> Result<i64> {
        let query = r#"SELECT COUNT(*) as count FROM "Users""#;
        let row = sqlx::query(query).fetch_one(&self.legacy_db).await?;
        Ok(row.try_get::<i64, _>("count").unwrap_or(0))
    }

    async fn get_current_user_count(&self) -> Result<i64> {
        let result = sqlx::query("SELECT COUNT(*) as count FROM user")
            .fetch_one(&self.current_db)
            .await?;
        Ok(result.try_get("count")?)
    }

    async fn get_legacy_payment_count(&mut self) -> Result<i64> {
        let query = r#"SELECT COUNT(*) as count FROM "Payments""#;
        let row = sqlx::query(query).fetch_one(&self.legacy_db).await?;
        Ok(row.try_get::<i64, _>("count").unwrap_or(0))
    }

    async fn get_current_payment_count(&self) -> Result<i64> {
        let result = sqlx::query("SELECT COUNT(*) as count FROM payment")
            .fetch_one(&self.current_db)
            .await?;
        Ok(result.try_get("count")?)
    }

    async fn get_legacy_stream_count(&mut self) -> Result<i64> {
        let query = r#"SELECT COUNT(*) as count FROM "Streams""#;
        let row = sqlx::query(query).fetch_one(&self.legacy_db).await?;
        Ok(row.try_get::<i64, _>("count").unwrap_or(0))
    }

    async fn get_current_stream_count(&self) -> Result<i64> {
        let result = sqlx::query("SELECT COUNT(*) as count FROM user_stream")
            .fetch_one(&self.current_db)
            .await?;
        Ok(result.try_get("count")?)
    }

    async fn get_legacy_stream_key_count(&mut self) -> Result<i64> {
        let query = r#"SELECT COUNT(*) as count FROM "StreamKeys""#;
        let row = sqlx::query(query).fetch_one(&self.legacy_db).await?;
        Ok(row.try_get::<i64, _>("count").unwrap_or(0))
    }

    async fn get_current_stream_key_count(&self) -> Result<i64> {
        let result = sqlx::query("SELECT COUNT(*) as count FROM user_stream_key")
            .fetch_one(&self.current_db)
            .await?;
        Ok(result.try_get("count")?)
    }

    async fn validate_user_balance(&mut self, pubkey: &str, user_id: u64) -> Result<()> {
        // Get current user balance
        let user_row = sqlx::query("SELECT balance FROM user WHERE id = ?")
            .bind(user_id)
            .fetch_one(&self.current_db)
            .await?;

        let current_balance: i64 = user_row.try_get("balance")?;

        // Get legacy user balance
        let legacy_row = sqlx::query(r#"SELECT "Balance" FROM "Users" WHERE "PubKey" = $1"#)
            .bind(pubkey)
            .fetch_one(&self.legacy_db)
            .await?;

        let legacy_balance: i64 = legacy_row.try_get("Balance")?;

        // Compare balances
        if current_balance == legacy_balance {
            println!(
                "✓ Balance validation passed for user {}: {} msat",
                pubkey, current_balance
            );
        } else {
            println!(
                "❌ Balance mismatch for user {}: legacy={} msat, current={} msat",
                pubkey, legacy_balance, current_balance
            );
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    println!("🚀 Starting migration tool...");
    println!("📋 Configuration:");
    println!("  Legacy connection: {}", args.legacy_connection);
    println!("  Current connection: {}", args.current_connection);
    println!("  Dry run: {}", args.dry_run);
    println!("  Validate only: {}", args.validate_only);

    let mut migration_tool = MigrationTool::new(
        &args.legacy_connection,
        &args.current_connection,
        args.dry_run,
    )
    .await?;

    if !args.validate_only {
        // Step 1: Migrate users and get mapping
        let pubkey_to_user_id = migration_tool.migrate_users().await?;

        // Step 2: Migrate payments using the user mapping
        migration_tool.migrate_payments(&pubkey_to_user_id).await?;

        // Step 3: Migrate streams using the user mapping
        migration_tool.migrate_streams(&pubkey_to_user_id).await?;

        // Step 4: Migrate stream keys using the user mapping
        migration_tool
            .migrate_stream_keys(&pubkey_to_user_id)
            .await?;

        // Step 5: Validate migration
        migration_tool
            .validate_migration(&pubkey_to_user_id)
            .await?;
    } else {
        // Just run validation
        let pubkey_to_user_id = HashMap::new(); // Would need to build this from current DB
        migration_tool
            .validate_migration(&pubkey_to_user_id)
            .await?;
    }

    println!("🎉 Migration completed successfully!");

    Ok(())
}
