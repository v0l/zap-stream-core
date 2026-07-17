//! Import tool for merging user data from another zap-stream-core instance.
//!
//! Imports user accounts, payment history and stream history between two
//! instances of the current MySQL schema (MySQL -> MySQL), and **merges** user
//! balances (adds the incoming balance on top of the local balance) rather than
//! overwriting them.
//!
//! Users are matched across systems by pubkey (their numeric ids differ), so
//! payments and streams are remapped onto the resolved local user id. Payment
//! hashes and stream ids are globally unique, so re-runs are naturally
//! deduplicated by primary key.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::Deserialize;
use sqlx::{FromRow, MySqlPool, Row};
use std::collections::HashMap;

#[derive(Parser, Debug)]
#[command(name = "import-tool")]
#[command(about = "Imports and merges user data from another zap-stream-core MySQL database")]
struct Args {
    /// MySQL connection string to import FROM
    #[arg(long = "from-connection")]
    from_connection: String,

    /// MySQL connection string to import INTO.
    /// If omitted, the primary database is read from the main config file
    /// (overseer.database).
    #[arg(long = "to-connection")]
    to_connection: Option<String>,

    /// Path to the main config file used to resolve the "to" database when
    /// --to-connection is not provided.
    #[arg(long = "config", default_value = "config.yaml")]
    config: String,

    /// Run in dry-run mode (no changes are written)
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Skip merging balances (only import history records)
    #[arg(long = "skip-balances")]
    skip_balances: bool,
}

/// Minimal view over the main config file, used only to resolve the primary
/// ("to") database connection string.
#[derive(Debug, Deserialize)]
struct ConfigOverseer {
    database: String,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    overseer: ConfigOverseer,
}

/// Resolve the "to" database connection: use the explicit flag if provided,
/// otherwise read `overseer.database` from the main config file.
fn resolve_to_connection(args: &Args) -> Result<String> {
    if let Some(conn) = &args.to_connection {
        return Ok(conn.clone());
    }

    let cfg: ConfigFile = config::Config::builder()
        .add_source(config::File::with_name(&args.config))
        .add_source(config::Environment::with_prefix("APP"))
        .build()
        .context("loading config file")?
        .try_deserialize()
        .context("parsing config file (overseer.database)")?;

    Ok(cfg.overseer.database)
}

#[derive(Debug, FromRow)]
struct FromUser {
    id: u64,
    pubkey: Vec<u8>,
    balance: i64,
    tos_accepted: Option<DateTime<Utc>>,
    stream_key: String,
    is_admin: bool,
    is_blocked: bool,
    recording: bool,
    stream_dump_recording: bool,
    title: Option<String>,
    summary: Option<String>,
    image: Option<String>,
    tags: Option<String>,
    content_warning: Option<String>,
    goal: Option<String>,
    nwc: Option<String>,
}

#[derive(Debug, FromRow)]
struct FromPayment {
    payment_hash: Vec<u8>,
    user_id: u64,
    invoice: Option<String>,
    is_paid: bool,
    amount: i64,
    created: DateTime<Utc>,
    expires: DateTime<Utc>,
    nostr: Option<String>,
    payment_type: u8,
    fee: u64,
    external_data: Option<String>,
}

#[derive(Debug, FromRow)]
struct FromStream {
    id: String,
    user_id: u64,
    starts: DateTime<Utc>,
    ends: Option<DateTime<Utc>>,
    state: u8,
    title: Option<String>,
    summary: Option<String>,
    image: Option<String>,
    thumb: Option<String>,
    tags: Option<String>,
    content_warning: Option<String>,
    goal: Option<String>,
    pinned: Option<String>,
    cost: u64,
    duration: f32,
    fee: Option<u32>,
    event: Option<String>,
    endpoint_id: Option<u64>,
    node_name: Option<String>,
    external_video_id: Option<String>,
    external_input_id: Option<String>,
}

#[derive(Debug, FromRow)]
struct FromStreamKey {
    user_id: u64,
    #[sqlx(rename = "key")]
    key: String,
    created: DateTime<Utc>,
    expires: Option<DateTime<Utc>>,
    stream_id: String,
    external_id: Option<String>,
}

struct ImportTool {
    from_db: MySqlPool,
    to_db: MySqlPool,
    dry_run: bool,
    skip_balances: bool,
}

impl ImportTool {
    async fn new(
        from_connection: &str,
        to_connection: &str,
        dry_run: bool,
        skip_balances: bool,
    ) -> Result<Self> {
        let from_db = MySqlPool::connect(from_connection)
            .await
            .context("connecting to 'from' database")?;
        let to_db = MySqlPool::connect(to_connection)
            .await
            .context("connecting to 'to' database")?;

        Ok(Self {
            from_db,
            to_db,
            dry_run,
            skip_balances,
        })
    }

    /// Import all users, returning a map from "from" user_id -> "to" user_id.
    async fn import_users(&self) -> Result<HashMap<u64, u64>> {
        println!("🔍 Fetching users from the 'from' database...");
        let users = sqlx::query_as::<_, FromUser>(
            "select id, pubkey, balance, tos_accepted, stream_key, is_admin, is_blocked, \
             recording, stream_dump_recording, title, summary, image, tags, \
             content_warning, goal, nwc from user",
        )
        .fetch_all(&self.from_db)
        .await?;
        println!("📊 Found {} users to import", users.len());

        let mut from_to: HashMap<u64, u64> = HashMap::new();

        for (i, user) in users.iter().enumerate() {
            // The "from" id is used to remap payments/streams onto the "to" user.
            let from_id = user.id;
            println!(
                "👤 Importing user {}/{}: {}",
                i + 1,
                users.len(),
                hex::encode(&user.pubkey)
            );
            match self.import_single_user(user).await {
                Ok(local_id) => {
                    from_to.insert(from_id, local_id);
                }
                Err(e) => {
                    println!(
                        "❌ Failed to import user {}: {}",
                        hex::encode(&user.pubkey),
                        e
                    );
                }
            }
        }

        println!("✅ User import completed ({} mapped)", from_to.len());
        Ok(from_to)
    }

    async fn import_single_user(&self, user: &FromUser) -> Result<u64> {
        let existing = sqlx::query("select id from user where pubkey = ?")
            .bind(user.pubkey.as_slice())
            .fetch_optional(&self.to_db)
            .await?;

        if self.dry_run {
            match existing {
                Some(row) => {
                    let id: u64 = row.try_get("id")?;
                    if !self.skip_balances && user.balance != 0 {
                        println!(
                            "  🔍 [DRY RUN] User {} exists (id {}), would add {} msat to balance",
                            hex::encode(&user.pubkey),
                            id,
                            user.balance
                        );
                    }
                    Ok(id)
                }
                None => {
                    println!(
                        "  🔍 [DRY RUN] Would create user {} with balance {} msat",
                        hex::encode(&user.pubkey),
                        user.balance
                    );
                    Ok(0)
                }
            }
        } else if let Some(row) = existing {
            // User already exists: only merge the balance (add on top). Existing
            // profile fields are left untouched.
            let local_id: u64 = row.try_get("id")?;
            if !self.skip_balances && user.balance != 0 {
                sqlx::query("update user set balance = balance + ? where id = ?")
                    .bind(user.balance)
                    .bind(local_id)
                    .execute(&self.to_db)
                    .await?;
                println!("  💰 Added {} msat to existing user {}", user.balance, local_id);
            }
            Ok(local_id)
        } else {
            // New user: insert with the incoming balance directly.
            let balance = if self.skip_balances { 0 } else { user.balance };
            let res = sqlx::query(
                "insert into user (pubkey, balance, tos_accepted, stream_key, is_admin, \
                 is_blocked, recording, stream_dump_recording, title, summary, image, tags, \
                 content_warning, goal, nwc) \
                 values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user.pubkey.as_slice())
            .bind(balance)
            .bind(user.tos_accepted)
            .bind(&user.stream_key)
            .bind(user.is_admin)
            .bind(user.is_blocked)
            .bind(user.recording)
            .bind(user.stream_dump_recording)
            .bind(&user.title)
            .bind(&user.summary)
            .bind(&user.image)
            .bind(&user.tags)
            .bind(&user.content_warning)
            .bind(&user.goal)
            .bind(&user.nwc)
            .execute(&self.to_db)
            .await?;
            println!("  ➕ Created user {} with balance {} msat", hex::encode(&user.pubkey), balance);
            Ok(res.last_insert_id())
        }
    }

    async fn import_payments(&self, from_to: &HashMap<u64, u64>) -> Result<()> {
        println!("💰 Fetching payments from the 'from' database...");
        let payments = sqlx::query_as::<_, FromPayment>(
            "select payment_hash, user_id, invoice, is_paid, amount, created, expires, \
             nostr, payment_type, fee, external_data from payment",
        )
        .fetch_all(&self.from_db)
        .await?;
        println!("📊 Found {} payments to import", payments.len());

        let mut imported = 0;
        for payment in &payments {
            let local_id = match from_to.get(&payment.user_id) {
                Some(id) => *id,
                None => continue,
            };
            match self.import_single_payment(payment, local_id).await {
                Ok(true) => imported += 1,
                Ok(false) => {}
                Err(e) => println!("❌ Failed to import payment: {}", e),
            }
        }

        println!("✅ Payment import completed ({} new)", imported);
        Ok(())
    }

    /// Returns Ok(true) if a new payment was inserted, Ok(false) if skipped.
    async fn import_single_payment(&self, payment: &FromPayment, local_id: u64) -> Result<bool> {
        if self.dry_run {
            println!(
                "  🔍 [DRY RUN] Would import payment {} ({} msat)",
                hex::encode(&payment.payment_hash),
                payment.amount
            );
            return Ok(false);
        }

        let existing = sqlx::query("select payment_hash from payment where payment_hash = ?")
            .bind(payment.payment_hash.as_slice())
            .fetch_optional(&self.to_db)
            .await?;
        if existing.is_some() {
            return Ok(false);
        }

        sqlx::query(
            "insert into payment (payment_hash, user_id, invoice, is_paid, amount, created, \
             expires, nostr, payment_type, fee, external_data) \
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(payment.payment_hash.as_slice())
        .bind(local_id)
        .bind(&payment.invoice)
        .bind(payment.is_paid)
        .bind(payment.amount)
        .bind(payment.created)
        .bind(payment.expires)
        .bind(&payment.nostr)
        .bind(payment.payment_type)
        .bind(payment.fee)
        .bind(&payment.external_data)
        .execute(&self.to_db)
        .await?;
        Ok(true)
    }

    /// Build a map from "from" ingest_endpoint id -> "to" ingest_endpoint id,
    /// matched by endpoint name. Unmatched endpoints map to None (endpoint_id
    /// is nulled on the imported stream to avoid FK violations).
    async fn build_endpoint_map(&self) -> Result<HashMap<u64, u64>> {
        let from_endpoints: Vec<(u64, String)> = sqlx::query("select id, name from ingest_endpoint")
            .fetch_all(&self.from_db)
            .await?
            .into_iter()
            .map(|r| (r.get::<u64, _>("id"), r.get::<String, _>("name")))
            .collect();
        let local: HashMap<String, u64> = sqlx::query("select id, name from ingest_endpoint")
            .fetch_all(&self.to_db)
            .await?
            .into_iter()
            .map(|r| (r.get::<String, _>("name"), r.get::<u64, _>("id")))
            .collect();

        let mut map = HashMap::new();
        for (from_id, name) in from_endpoints {
            if let Some(local_id) = local.get(&name) {
                map.insert(from_id, *local_id);
            }
        }
        Ok(map)
    }

    async fn import_streams(&self, from_to: &HashMap<u64, u64>) -> Result<()> {
        println!("🎬 Fetching streams from the 'from' database...");
        let streams = sqlx::query_as::<_, FromStream>(
            "select id, user_id, starts, ends, state, title, summary, image, thumb, tags, \
             content_warning, goal, pinned, cost, duration, fee, event, endpoint_id, \
             node_name, external_video_id, external_input_id from user_stream",
        )
        .fetch_all(&self.from_db)
        .await?;
        println!("📊 Found {} streams to import", streams.len());

        let endpoint_map = self.build_endpoint_map().await?;

        let mut imported = 0;
        for stream in &streams {
            let local_id = match from_to.get(&stream.user_id) {
                Some(id) => *id,
                None => continue,
            };
            match self
                .import_single_stream(stream, local_id, &endpoint_map)
                .await
            {
                Ok(true) => imported += 1,
                Ok(false) => {}
                Err(e) => println!("❌ Failed to import stream {}: {}", stream.id, e),
            }
        }

        println!("✅ Stream import completed ({} new)", imported);
        Ok(())
    }

    /// Returns Ok(true) if a new stream was inserted, Ok(false) if skipped.
    async fn import_single_stream(
        &self,
        stream: &FromStream,
        local_id: u64,
        endpoint_map: &HashMap<u64, u64>,
    ) -> Result<bool> {
        if self.dry_run {
            println!("  🔍 [DRY RUN] Would import stream {}", stream.id);
            return Ok(false);
        }

        let existing = sqlx::query("select id from user_stream where id = ?")
            .bind(&stream.id)
            .fetch_optional(&self.to_db)
            .await?;
        if existing.is_some() {
            return Ok(false);
        }

        let endpoint_id = stream
            .endpoint_id
            .and_then(|id| endpoint_map.get(&id).copied());

        // stream_key_id is intentionally left NULL: it references
        // user_stream_key.id which differs across systems (circular FK).
        sqlx::query(
            "insert into user_stream (id, user_id, starts, ends, state, title, summary, image, \
             thumb, tags, content_warning, goal, pinned, cost, duration, fee, event, \
             endpoint_id, node_name, stream_key_id, external_video_id, external_input_id) \
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, null, ?, ?)",
        )
        .bind(&stream.id)
        .bind(local_id)
        .bind(stream.starts)
        .bind(stream.ends)
        .bind(stream.state)
        .bind(&stream.title)
        .bind(&stream.summary)
        .bind(&stream.image)
        .bind(&stream.thumb)
        .bind(&stream.tags)
        .bind(&stream.content_warning)
        .bind(&stream.goal)
        .bind(&stream.pinned)
        .bind(stream.cost)
        .bind(stream.duration)
        .bind(stream.fee)
        .bind(&stream.event)
        .bind(endpoint_id)
        .bind(&stream.node_name)
        .bind(&stream.external_video_id)
        .bind(&stream.external_input_id)
        .execute(&self.to_db)
        .await?;
        Ok(true)
    }

    async fn import_stream_keys(&self, from_to: &HashMap<u64, u64>) -> Result<()> {
        println!("🔑 Fetching stream keys from the 'from' database...");
        let keys = sqlx::query_as::<_, FromStreamKey>(
            "select user_id, `key`, created, expires, stream_id, external_id \
             from user_stream_key",
        )
        .fetch_all(&self.from_db)
        .await?;
        println!("📊 Found {} stream keys to import", keys.len());

        let mut imported = 0;
        for key in &keys {
            let local_id = match from_to.get(&key.user_id) {
                Some(id) => *id,
                None => continue,
            };
            match self.import_single_stream_key(key, local_id).await {
                Ok(true) => imported += 1,
                Ok(false) => {}
                Err(e) => println!("❌ Failed to import stream key: {}", e),
            }
        }

        println!("✅ Stream key import completed ({} new)", imported);
        Ok(())
    }

    /// Returns Ok(true) if a new key was inserted, Ok(false) if skipped.
    async fn import_single_stream_key(&self, key: &FromStreamKey, local_id: u64) -> Result<bool> {
        if self.dry_run {
            println!(
                "  🔍 [DRY RUN] Would import stream key for stream {}",
                key.stream_id
            );
            return Ok(false);
        }

        // Only import the key if its referenced stream exists locally (FK) and
        // the key isn't already present.
        let stream_exists = sqlx::query("select id from user_stream where id = ?")
            .bind(&key.stream_id)
            .fetch_optional(&self.to_db)
            .await?
            .is_some();
        if !stream_exists {
            return Ok(false);
        }

        let existing = sqlx::query("select id from user_stream_key where `key` = ?")
            .bind(&key.key)
            .fetch_optional(&self.to_db)
            .await?;
        if existing.is_some() {
            return Ok(false);
        }

        sqlx::query(
            "insert into user_stream_key (user_id, `key`, created, expires, stream_id, external_id) \
             values (?, ?, ?, ?, ?, ?)",
        )
        .bind(local_id)
        .bind(&key.key)
        .bind(key.created)
        .bind(key.expires)
        .bind(&key.stream_id)
        .bind(&key.external_id)
        .execute(&self.to_db)
        .await?;
        Ok(true)
    }

    async fn report(&self) -> Result<()> {
        let src_users: i64 = sqlx::query("select count(*) as c from user")
            .fetch_one(&self.from_db)
            .await?
            .try_get("c")?;
        let dst_users: i64 = sqlx::query("select count(*) as c from user")
            .fetch_one(&self.to_db)
            .await?
            .try_get("c")?;
        println!("📊 Users - from: {}, to: {}", src_users, dst_users);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let to_connection = resolve_to_connection(&args)?;

    println!("🚀 Starting import tool...");
    println!("📋 Configuration:");
    println!("  From connection: {}", args.from_connection);
    println!("  To connection: {}", to_connection);
    if args.to_connection.is_none() {
        println!("    (resolved from config file: {})", args.config);
    }
    println!("  Dry run: {}", args.dry_run);
    println!("  Skip balances: {}", args.skip_balances);

    let tool = ImportTool::new(
        &args.from_connection,
        &to_connection,
        args.dry_run,
        args.skip_balances,
    )
    .await?;

    // Order matters: users -> payments -> streams -> stream keys.
    let from_to = tool.import_users().await?;
    tool.import_payments(&from_to).await?;
    tool.import_streams(&from_to).await?;
    tool.import_stream_keys(&from_to).await?;
    tool.report().await?;

    println!("🎉 Import completed successfully!");
    Ok(())
}
