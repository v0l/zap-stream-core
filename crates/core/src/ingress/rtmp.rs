use crate::ingress::{BufferedReader, ConnectionInfo};
use crate::overseer::Overseer;
use crate::pipeline::runner::PipelineRunner;
use anyhow::{bail, Result};
use log::{error, info};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::time::Instant;
use uuid::Uuid;

const MAX_MEDIA_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit

#[derive(PartialEq, Eq, Clone, Hash)]
struct RtmpPublishedStream(String, String);

struct RtmpClient {
    socket: TcpStream,
    buffer: BufferedReader,
    session: ServerSession,
    msg_queue: VecDeque<ServerSessionResult>,
    reader_buf: [u8; 4096],
    pub published_stream: Option<RtmpPublishedStream>,
}

impl RtmpClient {
    pub fn new(socket: TcpStream) -> Result<Self> {
        socket.set_nonblocking(false)?;
        let cfg = ServerSessionConfig::new();
        let (ses, res) = ServerSession::new(cfg)?;
        Ok(Self {
            socket,
            session: ses,
            buffer: BufferedReader::new(1024 * 1024, MAX_MEDIA_BUFFER_SIZE, "RTMP"),
            msg_queue: VecDeque::from(res),
            reader_buf: [0; 4096],
            published_stream: None,
        })
    }

    pub fn handshake(&mut self) -> Result<()> {
        let mut hs = Handshake::new(PeerType::Server);

        let exchange = hs.generate_outbound_p0_and_p1()?;
        self.socket.write_all(&exchange)?;

        let mut buf = [0; 4096];
        loop {
            let r = self.socket.read(&mut buf)?;
            if r == 0 {
                bail!("EOF reached while reading");
            }

            match hs.process_bytes(&buf[..r])? {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    self.socket.write_all(&response_bytes)?;
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    self.socket.write_all(&response_bytes)?;

                    let q = self.session.handle_input(&remaining_bytes)?;
                    self.msg_queue.extend(q);
                    return Ok(());
                }
            }
        }
    }

    /// Read data until we get the publish request
    pub fn read_until_publish_request(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        while self.published_stream.is_none() {
            if (Instant::now() - start) > timeout {
                bail!("Timed out waiting for publish request");
            }
            self.read_data()?;
        }
        Ok(())
    }

    fn read_data(&mut self) -> Result<()> {
        let r = match self.socket.read(&mut self.reader_buf) {
            Ok(r) => r,
            Err(e) => {
                return match e.kind() {
                    ErrorKind::WouldBlock => Ok(()),
                    ErrorKind::Interrupted => Ok(()),
                    _ => Err(anyhow::Error::new(e)),
                };
            }
        };
        if r == 0 {
            bail!("EOF");
        }

        let mx = self.session.handle_input(&self.reader_buf[..r])?;
        if !mx.is_empty() {
            self.msg_queue.extend(mx);
            self.process_msg_queue()?;
        }
        Ok(())
    }

    fn process_msg_queue(&mut self) -> Result<()> {
        while let Some(msg) = self.msg_queue.pop_front() {
            match msg {
                ServerSessionResult::OutboundResponse(data) => {
                    self.socket.write_all(&data.bytes)?
                }
                ServerSessionResult::RaisedEvent(ev) => self.handle_event(ev)?,
                ServerSessionResult::UnhandleableMessageReceived(m) => {
                    // Log unhandleable messages for debugging
                    error!("Received unhandleable message with {} bytes", m.data.len());
                }
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, event: ServerSessionEvent) -> Result<()> {
        match event {
            ServerSessionEvent::ClientChunkSizeChanged { new_chunk_size } => {
                info!("New client chunk size: {}", new_chunk_size);
            }
            ServerSessionEvent::ConnectionRequested { request_id, .. } => {
                let mx = self.session.accept_request(request_id)?;
                self.msg_queue.extend(mx);
            }
            ServerSessionEvent::ReleaseStreamRequested { .. } => {}
            ServerSessionEvent::PublishStreamRequested {
                request_id,
                app_name,
                stream_key,
                mode,
            } => {
                if self.published_stream.is_some() {
                    let mx =
                        self.session
                            .reject_request(request_id, "0", "stream already published")?;
                    self.msg_queue.extend(mx);
                } else {
                    let mx = self.session.accept_request(request_id)?;
                    self.msg_queue.extend(mx);
                    info!(
                        "Published stream request: {app_name}/{stream_key} [{:?}]",
                        mode
                    );
                    self.published_stream = Some(RtmpPublishedStream(app_name, stream_key));
                }
            }
            ServerSessionEvent::PublishStreamFinished { .. } => {}
            ServerSessionEvent::StreamMetadataChanged {
                app_name,
                stream_key,
                metadata,
            } => {
                info!(
                    "Metadata configured: {}/{} {:?}",
                    app_name, stream_key, metadata
                );
            }
            ServerSessionEvent::AudioDataReceived { data, .. } => {
                self.buffer.add_data(&data);
            }
            ServerSessionEvent::VideoDataReceived { data, .. } => {
                self.buffer.add_data(&data);
            }
            ServerSessionEvent::UnhandleableAmf0Command { .. } => {}
            ServerSessionEvent::PlayStreamRequested { request_id, .. } => {
                let mx = self
                    .session
                    .reject_request(request_id, "0", "playback not supported")?;
                self.msg_queue.extend(mx);
            }
            ServerSessionEvent::PlayStreamFinished { .. } => {}
            ServerSessionEvent::AcknowledgementReceived { .. } => {}
            ServerSessionEvent::PingResponseReceived { .. } => {}
        }
        Ok(())
    }
}

impl Read for RtmpClient {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Block until we have enough data to fill the buffer
        while self.buffer.buf.len() < buf.len() {
            if let Err(e) = self.read_data() {
                error!("Error reading data: {}", e);
                return Ok(0);
            };
        }

        Ok(self.buffer.read_buffered(buf))
    }
}

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("RTMP listening on: {}", &addr);
    while let Ok((socket, ip)) = listener.accept().await {
        let mut cc = RtmpClient::new(socket.into_std()?)?;
        let addr = addr.clone();
        let overseer = overseer.clone();
        let out_dir = out_dir.clone();
        let handle = Handle::current();
        std::thread::Builder::new()
            .name("rtmp-client".to_string())
            .spawn(move || {
                if let Err(e) = cc.handshake() {
                    bail!("Error during handshake: {}", e)
                }
                if let Err(e) = cc.read_until_publish_request(Duration::from_secs(10)) {
                    bail!("Error waiting for publish request: {}", e)
                }

                let pr = cc.published_stream.as_ref().unwrap();
                let info = ConnectionInfo {
                    id: Uuid::new_v4(),
                    ip_addr: ip.to_string(),
                    endpoint: addr.clone(),
                    app_name: pr.0.clone(),
                    key: pr.1.clone(),
                };
                let mut pl = match PipelineRunner::new(
                    handle,
                    out_dir,
                    overseer,
                    info,
                    Box::new(cc),
                    Some("flv".to_string()),
                ) {
                    Ok(pl) => pl,
                    Err(e) => {
                        bail!("Failed to create PipelineRunner {}", e)
                    }
                };
                pl.run();
                Ok(())
            })?;
    }
    Ok(())
}
