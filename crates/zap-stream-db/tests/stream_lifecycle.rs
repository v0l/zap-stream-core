//! Integration tests for stream DB operations against a real MariaDB.
//!
//! Each test spins up its own throwaway MariaDB container via the Docker API
//! (using the `bollard` crate), runs the embedded migrations, exercises the DB
//! layer, then tears the container down again on drop.
//!
//! A running Docker daemon is required — if it is not reachable the tests
//! panic (they do not silently skip).
//!
//! Run with: `cargo test -p zap-stream-db`

use bollard::container::{Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding};
use bollard::Docker;
use chrono::{Duration, Utc};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration as StdDuration, Instant};
use zap_stream_db::{UserStream, UserStreamState, ZapStreamDb};

const DB_IMAGE_NAME: &str = "mariadb";
const DB_IMAGE_TAG: &str = "lts";
const ROOT_PASSWORD: &str = "root";
const READY_TIMEOUT: StdDuration = StdDuration::from_secs(60);

static CONTAINER_SEQ: AtomicU32 = AtomicU32::new(0);

/// A MariaDB container that is force-removed on drop.
struct TestDb {
    docker: Docker,
    container_id: String,
    /// Connected, migrated handle to the DB.
    db: ZapStreamDb,
}

impl TestDb {
    /// Start a fresh MariaDB container, wait for it to accept connections,
    /// create the database and run all migrations.
    ///
    /// Panics if the Docker daemon is not reachable.
    async fn start() -> Self {
        let docker = Docker::connect_with_local_defaults()
            .expect("failed to construct Docker client — is Docker installed?");
        docker
            .ping()
            .await
            .expect("Docker daemon is not running — these tests require Docker");

        // Ensure the image is present.
        let mut pull = docker.create_image(
            Some(CreateImageOptions {
                from_image: DB_IMAGE_NAME,
                tag: DB_IMAGE_TAG,
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(step) = pull.next().await {
            step.expect("failed to pull mariadb image");
        }

        let seq = CONTAINER_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("zsc-db-test-{}-{}", std::process::id(), seq);

        // Expose 3306 and let Docker pick a free host port (host_port = "0").
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
        exposed_ports.insert("3306/tcp".to_string(), HashMap::new());

        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "3306/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some("0".to_string()),
            }]),
        );

        let config = Config {
            image: Some(format!("{DB_IMAGE_NAME}:{DB_IMAGE_TAG}")),
            env: Some(vec![format!("MARIADB_ROOT_PASSWORD={ROOT_PASSWORD}")]),
            exposed_ports: Some(exposed_ports),
            host_config: Some(HostConfig {
                port_bindings: Some(port_bindings),
                ..Default::default()
            }),
            ..Default::default()
        };

        let created = docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.as_str(),
                    platform: None,
                }),
                config,
            )
            .await
            .expect("failed to create MariaDB container");
        let container_id = created.id;

        docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .expect("failed to start MariaDB container");

        // Resolve the mapped host port.
        let host_port = resolve_host_port(&docker, &container_id).await;
        let server_url = format!("mysql://root:{ROOT_PASSWORD}@127.0.0.1:{host_port}");

        // Wait for the server to accept connections, then create + migrate the DB.
        let pool = wait_for_ready(&server_url, &docker, &container_id).await;
        sqlx::query("CREATE DATABASE IF NOT EXISTS zap_stream_test")
            .execute(&pool)
            .await
            .expect("create database");
        pool.close().await;

        let db = ZapStreamDb::new(&format!("{server_url}/zap_stream_test"))
            .await
            .expect("connect to zap_stream_test");
        db.migrate().await.expect("run migrations");

        TestDb {
            docker,
            container_id,
            db,
        }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // Drop runs synchronously and possibly on a tokio worker thread, so do
        // the async removal on a dedicated thread with its own runtime.
        let docker = self.docker.clone();
        let id = self.container_id.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            rt.block_on(async {
                let _ = docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await;
            });
        })
        .join();
    }
}

/// Read the host port that container port 3306 was mapped to.
async fn resolve_host_port(docker: &Docker, container_id: &str) -> u16 {
    let info = docker
        .inspect_container(container_id, None)
        .await
        .expect("inspect container");
    let ports = info
        .network_settings
        .and_then(|n| n.ports)
        .expect("container has no network ports");
    let binding = ports
        .get("3306/tcp")
        .cloned()
        .flatten()
        .expect("3306/tcp not bound");
    binding
        .first()
        .and_then(|b| b.host_port.clone())
        .expect("no host port assigned")
        .parse()
        .expect("host port not a number")
}

/// Poll until the MariaDB server accepts connections, or panic on timeout.
async fn wait_for_ready(
    server_url: &str,
    docker: &Docker,
    container_id: &str,
) -> sqlx::MySqlPool {
    use sqlx::mysql::MySqlPoolOptions;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(StdDuration::from_secs(2))
            .connect(server_url)
            .await
        {
            Ok(pool) => return pool,
            Err(e) => {
                if Instant::now() >= deadline {
                    let _ = docker
                        .remove_container(
                            container_id,
                            Some(RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        )
                        .await;
                    panic!("MariaDB did not become ready within {READY_TIMEOUT:?}: {e}");
                }
                tokio::time::sleep(StdDuration::from_millis(500)).await;
            }
        }
    }
}

/// Build a Live stream owned by `user_id` with the given external video id.
fn live_stream(user_id: u64, external_video_id: Option<&str>) -> UserStream {
    UserStream {
        id: uuid::Uuid::new_v4().to_string(),
        user_id,
        starts: Utc::now(),
        state: UserStreamState::Live,
        node_name: Some("test-node".to_string()),
        external_video_id: external_video_id.map(|s| s.to_string()),
        ..Default::default()
    }
}

/// Regression test for the 30311 relay-spam bug (issue #83).
///
/// `update_stream` had its last two parameter binds swapped, so the query
/// effectively became `... SET external_id = <id> WHERE id = <external_id>`.
/// With a NULL `external_id` (the common case) the WHERE matched no rows and
/// the update was a silent no-op — a stream could never transition to Ended,
/// so the background worker re-ended and re-published it forever.
#[tokio::test]
async fn ending_stream_with_null_external_id_persists() {
    let test = TestDb::start().await;
    let db = &test.db;

    let user_id = db.upsert_user(&[1u8; 32]).await.unwrap();

    let mut stream = live_stream(user_id, None);
    db.insert_stream(&stream).await.unwrap();

    let id = uuid::Uuid::parse_str(&stream.id).unwrap();
    assert_eq!(
        db.list_live_streams_by_node("test-node").await.unwrap().len(),
        1,
        "stream should be live after insert"
    );

    // End the stream, exactly like ZapStreamOverseer::on_end
    stream.state = UserStreamState::Ended;
    stream.ends = Some(Utc::now());
    db.update_stream(&stream).await.unwrap();

    let persisted = db.get_stream(&id).await.unwrap();
    assert_eq!(
        persisted.state,
        UserStreamState::Ended,
        "state=Ended must persist (was the no-op bug)"
    );
    assert!(persisted.ends.is_some(), "ends timestamp must persist");

    assert!(
        db.list_live_streams_by_node("test-node")
            .await
            .unwrap()
            .is_empty(),
        "ended stream must drop out of the live-streams query"
    );
}

/// The update must also work (and target the right row) when the external ids
/// are set. Exercises both external columns to guard against column/bind
/// mismatches.
#[tokio::test]
async fn update_stream_round_trips_external_ids() {
    let test = TestDb::start().await;
    let db = &test.db;

    let user_id = db.upsert_user(&[2u8; 32]).await.unwrap();

    let mut stream = live_stream(user_id, Some("vid-123"));
    db.insert_stream(&stream).await.unwrap();
    let id = uuid::Uuid::parse_str(&stream.id).unwrap();

    stream.title = Some("updated title".to_string());
    stream.external_video_id = Some("vid-456".to_string());
    stream.external_input_id = Some("input-789".to_string());
    db.update_stream(&stream).await.unwrap();

    let persisted = db.get_stream(&id).await.unwrap();
    assert_eq!(persisted.title.as_deref(), Some("updated title"));
    assert_eq!(
        persisted.external_video_id.as_deref(),
        Some("vid-456"),
        "external_video_id must be written to the external_video_id column"
    );
    assert_eq!(
        persisted.external_input_id.as_deref(),
        Some("input-789"),
        "external_input_id must be written to the external_input_id column"
    );
}

/// Updating one stream must not touch a sibling whose `id` happens to equal
/// the first stream's `external_id` — guards against the swapped-bind class of
/// bug where the WHERE clause matched on the wrong column.
#[tokio::test]
async fn update_stream_only_affects_target_row() {
    let test = TestDb::start().await;
    let db = &test.db;

    let user_id = db.upsert_user(&[3u8; 32]).await.unwrap();

    let mut a = live_stream(user_id, None);
    let b = live_stream(user_id, None);
    // Make A's external video id collide with B's id.
    a.external_video_id = Some(b.id.clone());

    db.insert_stream(&a).await.unwrap();
    db.insert_stream(&b).await.unwrap();

    let a_id = uuid::Uuid::parse_str(&a.id).unwrap();
    let b_id = uuid::Uuid::parse_str(&b.id).unwrap();

    a.state = UserStreamState::Ended;
    a.ends = Some(Utc::now() - Duration::seconds(1));
    db.update_stream(&a).await.unwrap();

    assert_eq!(
        db.get_stream(&a_id).await.unwrap().state,
        UserStreamState::Ended,
        "target row A must update"
    );
    assert_eq!(
        db.get_stream(&b_id).await.unwrap().state,
        UserStreamState::Live,
        "sibling row B must be untouched"
    );
}
