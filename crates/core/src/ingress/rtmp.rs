use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::{bail, Result};
use log::{error, info, warn};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio::time::Instant;

const MAX_MEDIA_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit
#[derive(PartialEq, Eq, Clone, Hash)]
struct RtmpPublishedStream(String, String);

struct RtmpClient {
    socket: std::net::TcpStream,
    media_buf: Vec<u8>,
    session: ServerSession,
    msg_queue: VecDeque<ServerSessionResult>,
    reader_buf: [u8; 4096],
    pub published_stream: Option<RtmpPublishedStream>,
    last_buffer_log: Instant,
    bytes_processed: u64,
    frames_received: u64,
}

impl RtmpClient {
    /// Add data to media buffer with size limit to prevent unbounded growth
    fn add_to_media_buffer(&mut self, data: &[u8]) {
        if self.media_buf.len() + data.len() > MAX_MEDIA_BUFFER_SIZE {
            let bytes_to_drop = (self.media_buf.len() + data.len()) - MAX_MEDIA_BUFFER_SIZE;
            warn!("RTMP buffer full ({} bytes), dropping {} oldest bytes", 
                  self.media_buf.len(), bytes_to_drop);
            self.media_buf.drain(..bytes_to_drop);
        }
        self.media_buf.extend(data);
        
        // Update performance counters
        self.bytes_processed += data.len() as u64;
        self.frames_received += 1;
        
        // Log buffer status every 5 seconds
        if self.last_buffer_log.elapsed().as_secs() >= 5 {
            let buffer_util = (self.media_buf.len() as f32 / MAX_MEDIA_BUFFER_SIZE as f32) * 100.0;
            let elapsed = self.last_buffer_log.elapsed();
            let mbps = (self.bytes_processed as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000.0);
            let fps = self.frames_received as f64 / elapsed.as_secs_f64();
            
            info!(
                "RTMP ingress: {:.1} Mbps, {:.1} frames/sec, buffer: {}% ({}/{} bytes)",
                mbps, fps, buffer_util as u32, self.media_buf.len(), MAX_MEDIA_BUFFER_SIZE
            );
            
            // Reset counters
            self.last_buffer_log = Instant::now();
            self.bytes_processed = 0;
            self.frames_received = 0;
        }
    }

    async fn start(mut socket: TcpStream) -> Result<Self> {
        let mut hs = Handshake::new(PeerType::Server);

        let exchange = hs.generate_outbound_p0_and_p1()?;
        socket.write_all(&exchange).await?;

        let mut buf = [0; 4096];
        loop {
            let r = socket.read(&mut buf).await?;
            if r == 0 {
                bail!("EOF reached while reading");
            }

            match hs.process_bytes(&buf[..r])? {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    socket.write_all(&response_bytes).await?;
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    socket.write_all(&response_bytes).await?;

                    let cfg = ServerSessionConfig::new();
                    let (mut ses, mut res) = ServerSession::new(cfg)?;
                    let q = ses.handle_input(&remaining_bytes)?;
                    res.extend(q);

                    let ret = Self {
                        socket: socket.into_std()?,
                        media_buf: vec![],
                        session: ses,
                        msg_queue: VecDeque::from(res),
                        reader_buf: [0; 4096],
                        published_stream: None,
                        last_buffer_log: Instant::now(),
                        bytes_processed: 0,
                        frames_received: 0,
                    };

                    return Ok(ret);
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
                    // Only append data if it looks like valid media data
                    if !m.data.is_empty() && m.data.len() > 4 {
                        self.add_to_media_buffer(&m.data);
                    }
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
                // Validate audio data before adding to buffer
                if !data.is_empty() {
                    self.add_to_media_buffer(&data);
                } else {
                    error!("Received empty audio data");
                }
            }
            ServerSessionEvent::VideoDataReceived { data, .. } => {
                // Validate video data before adding to buffer  
                if !data.is_empty() {
                    self.add_to_media_buffer(&data);
                } else {
                    error!("Received empty video data");
                }
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
        // block this thread until something comes into [media_buf]
        while self.media_buf.is_empty() {
            if let Err(e) = self.read_data() {
                error!("Error reading data: {}", e);
                return Ok(0);
            };
        }

        let to_read = buf.len().min(self.media_buf.len());
        let drain = self.media_buf.drain(..to_read);
        buf[..to_read].copy_from_slice(drain.as_slice());
        Ok(to_read)
    }
}

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("RTMP listening on: {}", &addr);
    while let Ok((socket, ip)) = listener.accept().await {
        let mut cc = RtmpClient::start(socket).await?;
        let addr = addr.clone();
        let overseer = overseer.clone();
        let out_dir = out_dir.clone();
        let handle = Handle::current();
        std::thread::Builder::new()
            .name("rtmp-client".to_string())
            .spawn(move || {
                if let Err(e) = cc.read_until_publish_request(Duration::from_secs(10)) {
                    error!("{}", e);
                } else {
                    let pr = cc.published_stream.as_ref().unwrap();
                    let info = ConnectionInfo {
                        ip_addr: ip.to_string(),
                        endpoint: addr.clone(),
                        app_name: pr.0.clone(),
                        key: pr.1.clone(),
                    };
                    spawn_pipeline(
                        handle,
                        info,
                        out_dir.clone(),
                        overseer.clone(),
                        Box::new(cc),
                    );
                }
            })?;
    }
    Ok(())
}
