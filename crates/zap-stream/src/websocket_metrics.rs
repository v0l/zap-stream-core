use anyhow::Result;
use axum::Router;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::routing::any;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use zap_stream::stream_manager::{ActiveStreamInfo, NodeInfo, StreamManager, StreamManagerMetric};
use zap_stream_api_common::Nip98Auth;
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
    /// Metrics for each worker node
    NodeMetrics(NodeInfo),
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

#[derive(Clone)]
pub struct WebSocketMetricsServer {
    db: ZapStreamDb,
    stream_manager: StreamManager,
}

#[derive(Clone)]
struct ClientSession {
    id: Uuid,
    is_admin: bool,
    user_id: Option<u64>,
    subscriptions: HashSet<String>,
}

impl WebSocketMetricsServer {
    pub fn new(db: ZapStreamDb, stream_manager: StreamManager) -> Router {
        Router::new()
            .route(
                "/api/v1/ws",
                any(async |ws: WebSocketUpgrade, State(this): State<Self>| {
                    ws.on_upgrade(async |w| {
                        if let Err(e) = Self::handle_websocket_connection(w, this).await {
                            warn!("Websocket handler failed {}", e);
                        }
                    })
                }),
            )
            .with_state(Self { db, stream_manager })
    }

    async fn handle_websocket_connection(websocket: WebSocket, this: Self) -> Result<()> {
        let (mut ws_sender, mut ws_receiver) = websocket.split();

        let mut session = ClientSession {
            id: Uuid::new_v4(),
            is_admin: false,
            user_id: None,
            subscriptions: HashSet::new(),
        };

        info!("WebSocket connection established: {}", session.id);
        let mut metrics = this.stream_manager.listen_metrics();
        loop {
            tokio::select! {
                // Handle incoming WebSocket messages
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Err(e) = Self::handle_client_message(
                                &text,
                                &mut session,
                                &this.db,
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
                                let ws_msg = match metric_msg {
                                    StreamManagerMetric::ActiveStream(s) => MetricMessage::StreamMetrics(s),
                                    StreamManagerMetric::Node(n) => MetricMessage::NodeMetrics(n)
                                };
                                let json = serde_json::to_string(&ws_msg)?;
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
            }
        }

        info!("WebSocket connection ended: {}", session.id);
        Ok(())
    }

    async fn handle_client_message(
        text: &str,
        session: &mut ClientSession,
        db: &ZapStreamDb,
        ws_sender: &mut SplitSink<WebSocket, Message>,
    ) -> Result<()> {
        let message: MetricMessage = serde_json::from_str(text)?;

        async fn send_reply<T: Serialize>(
            ws_sender: &mut SplitSink<WebSocket, Message>,
            msg: T,
        ) -> Result<()> {
            let json = serde_json::to_string(&msg)?;
            ws_sender
                .send(Message::Text(Utf8Bytes::from(&json)))
                .await?;
            Ok(())
        }
        async fn send_error(
            ws_sender: &mut SplitSink<WebSocket, Message>,
            msg: &str,
        ) -> Result<()> {
            let response = MetricMessage::Error {
                message: msg.to_string(),
            };
            send_reply(ws_sender, response).await
        }
        match message {
            MetricMessage::Auth { token } => {
                let auth = Nip98Auth::try_from_token(&token)?;
                if auth.method_tag != "GET" {
                    return send_error(ws_sender, "Invalid request method").await;
                }
                if !(auth.url_tag.starts_with("ws://") || auth.url_tag.starts_with("wss://"))
                    && !auth.url_tag.contains("/ws")
                {
                    return send_error(ws_sender, "Invalid auth URL tag").await;
                }

                let uid = db.upsert_user(&auth.pubkey).await?;
                let is_admin = db.is_admin(uid).await?;
                session.is_admin = is_admin;
                session.user_id = Some(uid);

                let response = MetricMessage::AuthResponse {
                    success: true,
                    is_admin,
                    pubkey: hex::encode(&auth.pubkey),
                };
                send_reply(ws_sender, response).await?;
                info!(
                    "Client {} authenticated successfully as {}",
                    session.id,
                    if is_admin { "admin" } else { "user" }
                );
            }
            MetricMessage::SubscribeStream { stream_id } => {
                if let Some(uid) = &session.user_id {
                    // Check if user can access this stream
                    let can_access = if session.is_admin {
                        // Admins can access any stream
                        true
                    } else {
                        // Regular users can only access their own streams
                        Self::verify_stream_ownership(&stream_id, *uid, db)
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
                        send_error(
                            ws_sender,
                            "Access denied: You can only access your own streams",
                        )
                        .await?;
                    }
                } else {
                    send_error(ws_sender, "Authentication required").await?;
                }
            }
            MetricMessage::SubscribeOverall => {
                if session.is_admin {
                    session.subscriptions.insert("all".to_string());
                    debug!("Client {} subscribed to overall metrics", session.id);
                } else {
                    send_error(ws_sender, "Admin access required for overall metrics").await?;
                }
            }
            _ => {
                send_error(ws_sender, "Invalid message type").await?;
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

    fn should_send_metric(session: &ClientSession, metric: &StreamManagerMetric) -> bool {
        match metric {
            StreamManagerMetric::ActiveStream(metric) => {
                session.subscriptions.contains(&metric.stream_id)
                    || session.subscriptions.contains("all")
            }
            StreamManagerMetric::Node(_) => session.subscriptions.contains("all"),
        }
    }
}
