use crate::ingress::{BufferedReader, ConnectionInfo};
use crate::overseer::Overseer;
use crate::pipeline::runner::{PipelineCommand, PipelineRunner};
use anyhow::{anyhow, bail, Result};
use bytes::{Bytes, BytesMut};
use log::{error, info, warn};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::time::Instant;
use uuid::Uuid;
use xflv::errors::FlvMuxerError;
use xflv::muxer::FlvMuxer;

const MAX_MEDIA_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10MB limit

#[derive(PartialEq, Eq, Clone, Hash)]
struct RtmpPublishedStream(String, String);

struct RtmpClient {
    socket: TcpStream,
    buffer: BufferedReader,
    session: ServerSession,
    msg_queue: VecDeque<ServerSessionResult>,
    pub published_stream: Option<RtmpPublishedStream>,
    muxer: FlvMuxer,
    tx: Sender<PipelineCommand>,
}

impl RtmpClient {
    pub fn new(socket: TcpStream, tx: Sender<PipelineCommand>) -> Result<Self> {
        socket.set_nonblocking(false)?;
        let cfg = ServerSessionConfig::new();
        let (ses, res) = ServerSession::new(cfg)?;
        Ok(Self {
            socket,
            session: ses,
            buffer: BufferedReader::new(1024 * 1024, MAX_MEDIA_BUFFER_SIZE, "RTMP"),
            msg_queue: VecDeque::from(res),
            published_stream: None,
            muxer: FlvMuxer::new(),
            tx,
        })
    }

    /// Read data until we get the publish request
    pub fn read_until_publish_request(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        let mut hs = Handshake::new(PeerType::Server);
        let mut handshake_complete = false;
        while self.published_stream.is_none() {
            if (Instant::now() - start) > timeout {
                bail!("Timed out waiting for publish request");
            }

            if let Some(data) = self.read_data()? {
                if !handshake_complete {
                    match hs.process_bytes(&data)? {
                        HandshakeProcessResult::InProgress { response_bytes } => {
                            if response_bytes.len() > 0 {
                                self.socket.write_all(&response_bytes)?;
                            }
                        }
                        HandshakeProcessResult::Completed {
                            response_bytes,
                            remaining_bytes,
                        } => {
                            if response_bytes.len() > 0 {
                                self.socket.write_all(&response_bytes)?;
                            }
                            if remaining_bytes.len() > 0 {
                                self.process_bytes(&remaining_bytes)?;
                            }
                            handshake_complete = true;
                        }
                    }
                } else {
                    self.process_bytes(&data)?;
                }
            }
        }
        Ok(())
    }

    fn read_data(&mut self) -> Result<Option<Vec<u8>>> {
        let mut buf = [0; 4096];
        let r = match self.socket.read(&mut buf) {
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
            return Ok(Some(Vec::new()));
        }

        Ok(Some(buf[..r].to_vec()))
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
                    let mx = self.session.accept_request(request_id)?;
                    self.msg_queue.extend(mx);
                    info!(
                        "Published stream request: {app_name}/{stream_key} [{:?}]",
                        mode
                    );
                    self.published_stream = Some(RtmpPublishedStream(app_name, stream_key));
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
                Ok(Some(data)) if data.len() == 0 => {
                    let r = self.buffer.read_buffered(buf);
                    if r == 0 {
                        return Err(std::io::Error::other(anyhow!("EOF")));
                    }
                    return Ok(r);
                }
                Ok(Some(data)) => {
                    if let Err(e) = self.process_bytes(&data) {
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

pub async fn listen(out_dir: String, addr: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    info!("RTMP listening on: {}", &addr);
    while let Ok((socket, ip)) = listener.accept().await {
        let overseer = overseer.clone();
        let out_dir = out_dir.clone();
        let handle = Handle::current();
        let new_id = Uuid::new_v4();
        std::thread::Builder::new()
            .name(format!("client:rtmp:{}", new_id))
            .spawn(move || {
                let (tx, rx) = std::sync::mpsc::channel();
                let mut cc = RtmpClient::new(socket.into_std()?, tx)?;
                if let Err(e) = cc.read_until_publish_request(Duration::from_secs(10)) {
                    bail!("Error waiting for publish request: {}", e)
                }

                let pr = cc.published_stream.as_ref().unwrap();
                let info = ConnectionInfo {
                    id: new_id,
                    ip_addr: ip.to_string(),
                    endpoint: "rtmp",
                    app_name: pr.0.clone(),
                    key: pr.1.clone(),
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
            })?;
    }
    Ok(())
}
