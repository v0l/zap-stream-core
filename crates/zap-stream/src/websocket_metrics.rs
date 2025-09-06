use crate::auth::{AuthRequest, TokenSource, authenticate_nip98};
use crate::stream_manager::{ActiveStreamInfo, StreamManager};
use anyhow::Result;
use bytes::Bytes;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Empty, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_tungstenite::{HyperWebsocket, tungstenite::Message};
use log::{debug, error, info, warn};
use nostr_sdk::{PublicKey, serde_json};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::broadcast;
use tungstenite::Utf8Bytes;
use uuid::Uuid;
use zap_stream_db::ZapStreamDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MetricMessage {
    /// Subscribe to specific stream metrics (streamer dashboard)
    SubscribeStream { stream_id: String },
    /// Subscribe to overall metrics (admin UI)
    SubscribeOverall,
    /// Stream-specific metrics
    StreamMetrics(ActiveStreamInfo),
    /// Overall system metrics
    OverallMetrics(OverallMetrics),
    /// Authentication request (NIP-98 token)
    Auth { token: String },
    /// Authentication response
    AuthResponse {
        success: bool,
        is_admin: bool,
        pubkey: String,
    },
    /// Error message
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallMetrics {
    pub total_streams: u64,
    pub total_viewers: u32,
    pub total_bandwidth: u64,
    pub cpu_load: f32,
    pub memory_load: f32,
    pub uptime_seconds: u64,
    pub timestamp: u64,
}

pub struct WebSocketMetricsServer {
    db: ZapStreamDb,
    stream_manager: StreamManager,
    public_url: String,
}

#[derive(Clone)]
struct ClientSession {
    id: Uuid,
    pubkey: Option<PublicKey>,
    is_admin: bool,
    user_id: Option<u64>,
    subscriptions: HashSet<String>,
}

impl WebSocketMetricsServer {
    pub fn new(db: ZapStreamDb, stream_manager: StreamManager, public_url: String) -> Self {
        Self {
            db,
            stream_manager,
            public_url,
        }
    }

    pub fn handle_websocket_upgrade(
        &self,
        request: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>> {
        if !hyper_tungstenite::is_upgrade_request(&request) {
            return Ok(Response::builder().status(StatusCode::BAD_REQUEST).body(
                Empty::<Bytes>::new()
                    .map_err(|e| anyhow::anyhow!("{}", e))
                    .boxed(),
            )?);
        }

        let (response, websocket) = hyper_tungstenite::upgrade(request, None)?;

        let db = self.db.clone();
        let public_url = self.public_url.clone();
        let stream_manager = self.stream_manager.clone();

        tokio::spawn(async move {
            if let Err(e) =
                Self::handle_websocket_connection(websocket, db, stream_manager, public_url).await
            {
                error!("WebSocket connection error: {}", e);
            }
        });

        Ok(response.map(|body| body.map_err(|e| anyhow::anyhow!("{}", e)).boxed()))
    }

    async fn handle_websocket_connection(
        websocket: HyperWebsocket,
        db: ZapStreamDb,
        stream_manager: StreamManager,
        public_url: String,
    ) -> Result<()> {
        let ws_stream = websocket.await?;
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        let mut session = ClientSession {
            id: Uuid::new_v4(),
            pubkey: None,
            is_admin: false,
            user_id: None,
            subscriptions: HashSet::new(),
        };

        info!("WebSocket connection established: {}", session.id);

        let mut timer = tokio::time::interval(Duration::from_secs(5));

        let mut metrics = stream_manager.listen_metrics();
        let mut sys = sysinfo::System::new();
        loop {
            tokio::select! {
                // Handle incoming WebSocket messages
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Err(e) = Self::handle_client_message(
                                &text,
                                &mut session,
                                &db,
                                &public_url,
                                &mut ws_sender
                            ).await {
                                error!("Error handling client message: {}", e);
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("WebSocket connection closed by client: {}", session.id);
                            break;
                        }
                        Some(Err(e)) => {
                            error!("WebSocket error: {}", e);
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }

                // Handle metric broadcasts
                metric = metrics.recv() => {
                    match metric {
                        Ok(metric_msg) => {
                            if Self::should_send_metric(&session, &metric_msg) {
                                let json = serde_json::to_string(&MetricMessage::StreamMetrics(metric_msg))?;
                                if let Err(e) = ws_sender.send(Message::Text(Utf8Bytes::from(&json))).await {
                                    error!("Failed to send metric: {}", e);
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            warn!("Client {} lagged behind metrics stream", session.id);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Metrics channel closed");
                            break;
                        }
                    }
                }

                // periodic overall stats
                _ = timer.tick() => {
                    if session.subscriptions.contains("all") {
                        sys.refresh_all();

                        let cpu_count = sys.cpus().len();
                        let all_streams = stream_manager.get_active_streams().await;

                        let mem = if let Some(cg) = sys.cgroup_limits() {
                            (cg.total_memory - cg.free_memory) as f64 / cg.total_memory as f64
                        } else {
                            sys.used_memory() as f64 / sys.total_memory() as f64
                        } as _;

                        let cpu = if let Some(p) = sys.process(sysinfo::get_current_pid().unwrap()) {
                            p.cpu_usage()
                        } else {
                            sys.global_cpu_usage()
                        } / cpu_count as f32 / 100.0;

                        let overall = OverallMetrics {
                            total_streams: all_streams.len() as _,
                            total_viewers: all_streams.iter().fold(0u32, |acc,v| acc + v.1.viewers),
                            total_bandwidth: all_streams.iter().fold(0u64, |acc,v| acc + v.1.endpoint_stats.values().fold(0u64, |acc2, v2| acc2 + v2.bitrate as u64)),
                            cpu_load: cpu,
                            memory_load: mem,
                            uptime_seconds: 0,
                            timestamp: Utc::now().timestamp() as _,
                        };
                        let json = serde_json::to_string(&MetricMessage::OverallMetrics(overall))?;
                        if let Err(e) = ws_sender.send(Message::Text(Utf8Bytes::from(&json))).await {
                            error!("Failed to send metric: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        info!("WebSocket connection ended: {}", session.id);
        Ok(())
    }

    async fn handle_client_message(
        text: &str,
        session: &mut ClientSession,
        db: &ZapStreamDb,
        public_url: &str,
        ws_sender: &mut futures_util::stream::SplitSink<
            hyper_tungstenite::WebSocketStream<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>,
            Message,
        >,
    ) -> Result<()> {
        let message: MetricMessage = serde_json::from_str(text)?;

        match message {
            MetricMessage::Auth { token } => {
                // For WebSocket, construct the expected URL
                let expected_url = format!(
                    "{}/api/v1/ws",
                    public_url.trim_end_matches('/').replace("http", "ws")
                );

                let auth_request = AuthRequest {
                    token_source: TokenSource::WebSocketToken(token),
                    expected_url: expected_url.parse()?,
                    expected_method: "GET".to_string(),
                    ignore_host: false,
                };

                match authenticate_nip98(auth_request, db).await {
                    Ok(auth_result) => {
                        session.pubkey = Some(auth_result.pubkey);
                        session.is_admin = auth_result.is_admin;
                        session.user_id = Some(auth_result.user_id);

                        let response = MetricMessage::AuthResponse {
                            success: true,
                            is_admin: auth_result.is_admin,
                            pubkey: auth_result.pubkey.to_string(),
                        };
                        let json = serde_json::to_string(&response)?;
                        ws_sender
                            .send(Message::Text(Utf8Bytes::from(&json)))
                            .await?;
                        info!(
                            "Client {} authenticated successfully as {}",
                            session.id,
                            if auth_result.is_admin {
                                "admin"
                            } else {
                                "user"
                            }
                        );
                    }
                    Err(e) => {
                        let response = MetricMessage::Error {
                            message: format!("Authentication failed: {}", e),
                        };
                        let json = serde_json::to_string(&response)?;
                        ws_sender
                            .send(Message::Text(Utf8Bytes::from(&json)))
                            .await?;
                        warn!("Authentication failed for client {}: {}", session.id, e);
                    }
                }
            }

            MetricMessage::SubscribeStream { stream_id } => {
                if let Some(_pubkey) = &session.pubkey {
                    // Check if user can access this stream
                    let can_access = if session.is_admin {
                        // Admins can access any stream
                        true
                    } else {
                        // Regular users can only access their own streams
                        Self::verify_stream_ownership(&stream_id, session.user_id.unwrap(), db)
                            .await
                            .unwrap_or_else(|e| {
                                warn!("Error checking stream ownership: {}", e);
                                false
                            })
                    };

                    if can_access {
                        session.subscriptions.insert(stream_id.clone());
                        debug!("Client {} subscribed to stream {}", session.id, stream_id);
                    } else {
                        let response = MetricMessage::Error {
                            message: "Access denied: You can only access your own streams"
                                .to_string(),
                        };
                        let json = serde_json::to_string(&response)?;
                        ws_sender
                            .send(Message::Text(Utf8Bytes::from(&json)))
                            .await?;
                    }
                } else {
                    let response = MetricMessage::Error {
                        message: "Authentication required".to_string(),
                    };
                    let json = serde_json::to_string(&response)?;
                    ws_sender
                        .send(Message::Text(Utf8Bytes::from(&json)))
                        .await?;
                }
            }

            MetricMessage::SubscribeOverall => {
                if session.is_admin {
                    session.subscriptions.insert("all".to_string());
                    debug!("Client {} subscribed to overall metrics", session.id);
                } else {
                    let response = MetricMessage::Error {
                        message: "Admin access required for overall metrics".to_string(),
                    };
                    let json = serde_json::to_string(&response)?;
                    ws_sender
                        .send(Message::Text(Utf8Bytes::from(&json)))
                        .await?;
                }
            }

            _ => {
                let response = MetricMessage::Error {
                    message: "Invalid message type".to_string(),
                };
                let json = serde_json::to_string(&response)?;
                ws_sender
                    .send(Message::Text(Utf8Bytes::from(&json)))
                    .await?;
            }
        }

        Ok(())
    }

    /// Verify if a user owns a specific stream
    async fn verify_stream_ownership(
        stream_id: &str,
        user_id: u64,
        db: &ZapStreamDb,
    ) -> Result<bool> {
        let stream_uuid = Uuid::parse_str(stream_id)?;
        match db.get_stream(&stream_uuid).await {
            Ok(stream) => Ok(stream.user_id == user_id),
            Err(_) => {
                // Stream doesn't exist or other error - deny access
                Ok(false)
            }
        }
    }

    fn should_send_metric(session: &ClientSession, metric: &ActiveStreamInfo) -> bool {
        session.subscriptions.contains(&metric.stream_id) || session.subscriptions.contains("all")
    }
}
