use crate::auth::{AuthRequest, TokenSource, authenticate_nip98};
use crate::settings::Settings;
use crate::stream_manager::{ActiveStreamInfo, NodeInfo, StreamManager, StreamManagerMetric};
use anyhow::Result;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Empty, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_tungstenite::{HyperWebsocket, tungstenite::Message};
use nostr_sdk::{PublicKey, serde_json};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
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

pub struct WebSocketMetricsServer {
    db: ZapStreamDb,
    stream_manager: StreamManager,
    settings: Settings,
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
    pub fn new(db: ZapStreamDb, stream_manager: StreamManager, settings: Settings) -> Self {
        Self {
            db,
            stream_manager,
            settings,
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
        let settings = self.settings.clone();
        let stream_manager = self.stream_manager.clone();

        tokio::spawn(async move {
            if let Err(e) =
                Self::handle_websocket_connection(websocket, db, stream_manager, settings).await
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
        settings: Settings,
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

        let mut metrics = stream_manager.listen_metrics();
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
                                &settings,
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
        settings: &Settings,
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
                    settings
                        .public_url
                        .trim_end_matches('/')
                        .replace("http", "ws")
                );

                let auth_request = AuthRequest {
                    token_source: TokenSource::WebSocketToken(token),
                    expected_url: expected_url.parse()?,
                    expected_method: "GET".to_string(),
                    skip_url_check: false,
                    admin_pubkey: settings.admin_pubkey.clone(),
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
