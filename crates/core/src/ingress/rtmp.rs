use crate::ingress::{BufferedReader, ConnectionInfo, setup_term_handler};
use crate::overseer::{ConnectResult, Overseer};
use crate::pipeline::runner::{PipelineCommand, PipelineRunner};
use anyhow::{Result, anyhow, bail};
use bytes::{Bytes, BytesMut};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;
use xflv::errors::FlvMuxerError;
use xflv::muxer::FlvMuxer;

const MAX_MEDIA_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit

#[derive(PartialEq, Eq, Clone, Hash)]
struct RtmpPublishedStream(String, String);

struct RtmpClient {
    socket: TcpStream,
    socket_buf: [u8; 4096],
    buffer: BufferedReader,
    session: ServerSession,
    msg_queue: VecDeque<ServerSessionResult>,
    pub published_stream: Option<RtmpPublishedStream>,
    muxer: FlvMuxer,
    tx: UnboundedSender<PipelineCommand>,
    // Handler to accept/deny publish requests
    publish_handler: Option<Box<dyn FnMut(&str, &str) -> (bool, Option<String>) + Send + 'static>>,
}

impl RtmpClient {
    pub fn new(socket: TcpStream, tx: UnboundedSender<PipelineCommand>) -> Result<Self> {
        socket.set_nonblocking(false)?;
        let cfg = ServerSessionConfig::new();
        let (ses, res) = ServerSession::new(cfg)?;
        Ok(Self {
            socket,
            session: ses,
            socket_buf: [0; 4096],
            buffer: BufferedReader::new(
                1024 * 1024,
                MAX_MEDIA_BUFFER_SIZE,
                "RTMP",
                Some(tx.clone()),
            ),
            msg_queue: VecDeque::from(res),
            published_stream: None,
            publish_handler: None,
            muxer: FlvMuxer::new(),
            tx,
        })
    }

    pub fn set_stream_dump_handle<W: Write + Send + 'static>(&mut self, dest: W) {
        self.buffer.set_dump_handle(dest);
    }

    pub fn set_publish_handler<F: FnMut(&str, &str) -> (bool, Option<String>) + Send + 'static>(
        &mut self,
        publish_handler: F,
    ) {
        self.publish_handler = Some(Box::new(publish_handler));
    }

    /// Read data until we get the publish request
    pub fn read_until_publish_request(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        let mut hs = Handshake::new(PeerType::Server);
        let mut handshake_complete = false;
        while self.published_stream.is_none() {
            if (Instant::now() - start) > timeout {
                // finish processing any messages in the queue
                self.process_msg_queue()?;
                bail!("Timed out waiting for publish request");
            }

            if let Some(r_len) = self.read_data()? {
                if r_len == 0 {
                    // finish processing any messages in the queue
                    self.process_msg_queue()?;
                    bail!("EOF while waiting for publish request");
                }
                if !handshake_complete {
                    let data = &self.socket_buf[..r_len];
                    match hs.process_bytes(data)? {
                        HandshakeProcessResult::InProgress { response_bytes } => {
                            if !response_bytes.is_empty() {
                                self.socket.write_all(&response_bytes)?;
                            }
                        }
                        HandshakeProcessResult::Completed {
                            response_bytes,
                            remaining_bytes,
                        } => {
                            if !response_bytes.is_empty() {
                                self.socket.write_all(&response_bytes)?;
                            }
                            if !remaining_bytes.is_empty() {
                                self.process_bytes(&remaining_bytes)?;
                            }
                            handshake_complete = true;
                        }
                    }
                } else {
                    self.process_socket_buf(r_len)?;
                }
            } else {
                // avoid spin loop
                sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    fn read_data(&mut self) -> Result<Option<usize>> {
        let r = match self.socket.read(&mut self.socket_buf) {
            Ok(r) => r,
            Err(e) => {
                return match e.kind() {
                    ErrorKind::WouldBlock => Ok(None),
                    ErrorKind::Interrupted => Ok(None),
                    _ => Err(anyhow::Error::new(e)),
                };
            }
        };
        if r == 0 {
            return Ok(Some(0));
        }

        Ok(Some(r))
    }

    fn process_socket_buf(&mut self, len: usize) -> Result<()> {
        let data = &self.socket_buf[..len];
        let msg = self.session.handle_input(data)?;
        if !msg.is_empty() {
            self.msg_queue.extend(msg);
        }
        self.process_msg_queue()
    }

    fn process_bytes(&mut self, data: &[u8]) -> Result<()> {
        let msg = self.session.handle_input(data)?;
        if !msg.is_empty() {
            self.msg_queue.extend(msg);
        }
        self.process_msg_queue()
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

    fn write_flv_header(&mut self, metadata: &rml_rtmp::sessions::StreamMetadata) -> Result<()> {
        let has_video = metadata.video_codec_id.is_some();
        let has_audio = metadata.audio_codec_id.is_some();

        self.muxer
            .write_flv_header(has_audio, has_video)
            .map_err(|e| anyhow!("failed to write flv header {}", e))?;
        self.muxer
            .write_previous_tag_size(0)
            .map_err(|e| anyhow!("failed to write flv header {}", e))?;

        // Extract data from the muxer
        let data = self.muxer.writer.extract_current_bytes();
        self.buffer.add_data(&data);

        info!(
            "FLV header written with audio: {}, video: {}",
            has_audio, has_video
        );
        Ok(())
    }

    fn write_flv_tag(
        &mut self,
        tag_type: u8,
        timestamp: u32,
        data: Bytes,
    ) -> Result<(), FlvMuxerError> {
        let body_len = data.len();
        self.muxer
            .write_flv_tag_header(tag_type, body_len as _, timestamp)?;
        self.muxer.write_flv_tag_body(BytesMut::from(data))?;
        self.muxer.write_previous_tag_size((11 + body_len) as _)?;
        let flv_data = self.muxer.writer.extract_current_bytes();
        self.buffer.add_data(&flv_data);
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
                    let (should_accept, msg) = if let Some(h) = &mut self.publish_handler {
                        h(&app_name, &stream_key)
                    } else {
                        (true, None)
                    };
                    if should_accept {
                        let mx = self.session.accept_request(request_id)?;
                        self.msg_queue.extend(mx);
                        info!(
                            "Published stream request: {app_name}/{stream_key} [{:?}]",
                            mode
                        );
                        self.published_stream = Some(RtmpPublishedStream(app_name, stream_key));
                    } else {
                        let msg = msg.unwrap_or("not allowed".to_string());
                        info!("Publish request was rejected for {app_name}/{stream_key}: {msg}");
                        let mx = self.session.reject_request(request_id, "0", &msg)?;
                        self.msg_queue.extend(mx);
                        self.socket.shutdown(Shutdown::Read)?; //half-close
                    }
                }
            }
            ServerSessionEvent::PublishStreamFinished {
                app_name,
                stream_key,
            } => {
                self.tx.send(PipelineCommand::Shutdown)?;
                info!("Stream ending: {app_name}/{stream_key}");
            }
            ServerSessionEvent::StreamMetadataChanged {
                app_name,
                stream_key,
                metadata,
            } => {
                info!(
                    "Metadata configured: {}/{} {:?}",
                    app_name, stream_key, metadata
                );
                self.write_flv_header(&metadata)?;
            }
            ServerSessionEvent::AudioDataReceived {
                data, timestamp, ..
            } => {
                self.write_flv_tag(8, timestamp.value, data)
                    .map_err(|e| anyhow!("failed to write flv tag: {}", e))?;
            }
            ServerSessionEvent::VideoDataReceived {
                data, timestamp, ..
            } => {
                self.write_flv_tag(9, timestamp.value, data)
                    .map_err(|e| anyhow!("failed to write flv tag: {}", e))?;
            }
            ServerSessionEvent::PlayStreamRequested { request_id, .. } => {
                let mx = self
                    .session
                    .reject_request(request_id, "0", "playback not supported")?;
                self.msg_queue.extend(mx);
            }
            e => warn!("Unhandled ServerSessionEvent: {:?}", e),
        }
        Ok(())
    }
}

impl Read for RtmpClient {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Block until we have enough data to fill the buffer
        while self.buffer.buf.len() < buf.len() {
            match self.read_data() {
                Ok(Some(r_len)) if r_len == 0 => {
                    let r = self.buffer.read_buffered(buf);
                    if r == 0 {
                        return Err(std::io::Error::other(anyhow!("EOF")));
                    }
                    return Ok(r);
                }
                Ok(Some(r_len)) => {
                    if let Err(e) = self.process_socket_buf(r_len) {
                        error!("Error processing bytes: {}", e);
                        return Ok(0);
                    }
                }
                Err(e) => {
                    error!("Error reading data: {}", e);
                    return Ok(0);
                }
                _ => continue,
            }
        }

        Ok(self.buffer.read_buffered(buf))
    }
}

pub async fn listen(
    out_dir: String,
    addr: String,
    overseer: Arc<dyn Overseer>,
    shutdown: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("RTMP listening on: {}", &addr);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                break;
            }
            Ok((socket, addr)) = listener.accept() => {
                let overseer = overseer.clone();
                let out_dir = PathBuf::from(out_dir.clone());

                let new_id = Uuid::new_v4();
                let shutdown = shutdown.clone();
                let handle = Handle::current();
                let (tx, rx) = unbounded_channel();
                setup_term_handler(shutdown, tx.clone());
                std::thread::Builder::new()
                    .name(format!("client:rtmp:{}", new_id))
                    .spawn(move || {
                    if let Err(e) = socket_handler(new_id, handle, socket, addr, out_dir, overseer, tx, rx) {
                        error!("Error handling RTMP socket: {}", e);
                    }
                })?;
            }
        }
    }

    info!("RTMP listener stopped.");
    Ok(())
}

fn socket_handler(
    id: Uuid,
    handle: Handle,
    socket: tokio::net::TcpStream,
    addr: SocketAddr,
    out_dir: PathBuf,
    overseer: Arc<dyn Overseer>,
    tx: UnboundedSender<PipelineCommand>,
    rx: UnboundedReceiver<PipelineCommand>,
) -> Result<()> {
    let mut cc = RtmpClient::new(socket.into_std()?, tx)?;
    let ip_addr = addr.to_string();
    let ov_pub = overseer.clone();
    let handle_pub = handle.clone();
    let dump_stream = Arc::new(Mutex::new(false));
    let id = Arc::new(Mutex::new(id));
    let id_pub = id.clone();
    let dump_stream_pub = dump_stream.clone();
    cc.set_publish_handler(move |app, key| {
        if app.is_empty() || key.is_empty() {
            return (false, Some("Invalid app or key".to_string()));
        }
        let info = ConnectionInfo {
            id: *id_pub.lock().unwrap(),
            endpoint: "rtmp",
            ip_addr: ip_addr.clone(),
            app_name: app.to_string(),
            key: key.to_string(),
        };
        match handle_pub.block_on(ov_pub.connect(&info)) {
            Ok(ConnectResult::Allow {
                stream_id_override,
                enable_stream_dump,
            }) => {
                *dump_stream_pub.lock().unwrap() = enable_stream_dump;
                if let Some(o) = stream_id_override {
                    *id_pub.lock().unwrap() = o;
                }
                (true, None)
            }
            Ok(ConnectResult::Deny { reason }) => {
                warn!("Connection denied: {reason}");
                (false, Some(reason))
            }
            Err(e) => (false, Some(format!("Failed to publish stream: {}", e))),
        }
    });

    if let Err(e) = cc.read_until_publish_request(Duration::from_secs(10)) {
        bail!("Error waiting for publish request: {}", e)
    }

    let id = *id.lock().unwrap();
    let out_dir = out_dir.join(id.to_string());
    if !out_dir.exists() {
        std::fs::create_dir_all(&out_dir)?;
    }

    if *dump_stream.lock().unwrap()
        && let Ok(f) = File::create(out_dir.join("stream.dump"))
    {
        cc.set_stream_dump_handle(f);
    }
    let pr = cc.published_stream.as_ref().unwrap();
    let info = ConnectionInfo {
        id,
        ip_addr: addr.to_string(),
        endpoint: "rtmp",
        app_name: pr.0.trim().to_string(),
        key: pr.1.trim().to_string(),
    };

    let mut pl = match PipelineRunner::new(
        handle,
        out_dir,
        overseer,
        info,
        Box::new(cc),
        None,
        Some(rx),
    ) {
        Ok(pl) => pl,
        Err(e) => {
            bail!("Failed to create PipelineRunner {}", e)
        }
    };
    pl.run();
    Ok(())
}
