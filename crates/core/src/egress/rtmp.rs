use crate::egress::{Egress, EgressResult, EncoderOrSourceStream};
use crate::mux;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{anyhow, bail, Context, Result};
use bytes::{Bytes, BytesMut};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{av_q2d, AVPacket};
use ffmpeg_rs_raw::Muxer;
use log::{error, info, trace, warn};
use rml_rtmp::chunk_io::Packet;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionError, ClientSessionEvent,
    ClientSessionResult, PublishRequestType, StreamMetadata,
};
use rml_rtmp::time::RtmpTimestamp;
use std::fmt::Display;
use std::io::Write;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use url::Url;
use uuid::Uuid;
use xflv::errors::FlvMuxerError;
use xflv::mpeg4_aac::Mpeg4AacProcessor;
use xflv::mpeg4_avc::Mpeg4AvcProcessor;
use xflv::muxer::FlvMuxer;

pub fn video_codec_id_to_name(codec_id: u8) -> Option<&'static str> {
    match codec_id {
        2 => Some("flv1"),
        3 => Some("flashsv"),
        4 => Some("vp6f"),
        5 => Some("vp6a"),
        6 => Some("flashsv2"),
        7 => Some("h264"),
        12 => Some("hevc"),
        _ => None,
    }
}

pub fn video_codec_name_to_id(codec_name: &str) -> Option<u8> {
    match codec_name {
        "flv1" => Some(2),
        "flashsv" => Some(3),
        "vp6f" => Some(4),
        "vp6a" => Some(5),
        "flashsv2" => Some(6),
        "h264" | "libx264" => Some(7),
        "hevc" | "libx265" => Some(12),
        _ => None,
    }
}

pub fn audio_codec_id_to_name(codec_id: u8) -> Option<&'static str> {
    match codec_id {
        0 => Some("pcm_s16be"),
        1 => Some("adpcm_swf"),
        2 => Some("mp3"),
        3 => Some("pcm_s16le"),
        4 => Some("nellymoser"),
        5 => Some("nellymoser"),
        6 => Some("nellymoser"),
        7 => Some("pcm_alaw"),
        8 => Some("pcm_mulaw"),
        10 => Some("aac"),
        11 => Some("speex"),
        14 => Some("mp3"),
        15 => Some("device_specific"),
        _ => None,
    }
}

pub fn audio_codec_name_to_id(codec_name: &str) -> Option<u8> {
    match codec_name {
        "pcm_s16be" => Some(0),
        "adpcm_swf" => Some(1),
        "mp3" | "libmp3lame" => Some(2),
        "pcm_s16le" => Some(3),
        "nellymoser" => Some(4),
        "pcm_alaw" => Some(7),
        "pcm_mulaw" => Some(8),
        "aac" | "libfdk_aac" => Some(10),
        "speex" | "libspeex" => Some(11),
        _ => None,
    }
}

enum ConnectionState {
    Connecting,
    RequestedConnection,
    Connected,
    RequestedPublish,
    PublishingMetadata,
    Publishing,
    Disconnected,
}

impl Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionState::Connecting => write!(f, "Connecting"),
            ConnectionState::Connected => write!(f, "Connected"),
            ConnectionState::Disconnected => write!(f, "Disconnected"),
            ConnectionState::Publishing => write!(f, "Publishing"),
            ConnectionState::RequestedPublish => write!(f, "RequestedPublish"),
            ConnectionState::RequestedConnection => write!(f, "RequestedConnection"),
            ConnectionState::PublishingMetadata => write!(f, "PublishingMetadata"),
        }
    }
}

/// Forwards RTMP stream to another server
pub struct RtmpEgress {
    state: ConnectionState,
    dest: String,
    session: ClientSession,
    metadata: StreamMetadata,
    video_variant: VariantStream,
    audio_variant: Option<VariantStream>,
    muxer: FlvMuxer,
    avc_processor: Mpeg4AvcProcessor,
    aac_processor: Mpeg4AacProcessor,

    out_tx: Option<UnboundedSender<ClientSessionResult>>,
    in_rx: Option<UnboundedReceiver<Vec<u8>>>,

    // Monotonic timestamp tracking
    video_timestamp: u32,
    audio_timestamp: u32,
}

struct StreamKey {
    pub app: String,
    pub key: String,
}

struct TxWriter {
    tx: UnboundedSender<ClientSessionResult>,
}

impl Write for TxWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tx
            .send(ClientSessionResult::OutboundResponse(Packet {
                bytes: buf.to_vec(),
                can_be_dropped: false,
            }))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl RtmpEgress {
    pub fn new<'a>(
        dst: &str,
        encoders: impl Iterator<Item = (&'a VariantStream, EncoderOrSourceStream<'a>)>,
    ) -> Result<Self> {
        let cfg = ClientSessionConfig::new();
        let (session, _) = ClientSession::new(cfg)?;

        let mut video_var = None;
        let mut audio_var = None;
        let mut metadata = StreamMetadata::new();
        metadata.encoder = Some("zap-stream-core".to_string());

        for (var, _enc) in encoders {
            match var {
                VariantStream::Video(v) => {
                    if metadata.video_codec_id.is_none() {
                        metadata.video_frame_rate = Some(v.fps);
                        metadata.video_height = Some(v.height as _);
                        metadata.video_width = Some(v.width as _);
                        metadata.video_bitrate_kbps = Some((v.bitrate / 1000) as _);
                        metadata.video_codec_id =
                            video_codec_name_to_id(&v.codec).map(|id| id as u32);
                        video_var = Some(var);
                    }
                }
                VariantStream::Audio(v) => {
                    if metadata.audio_codec_id.is_none() {
                        metadata.audio_sample_rate = Some(v.sample_rate as _);
                        metadata.audio_channels = Some(v.channels as _);
                        metadata.audio_bitrate_kbps = Some((v.bitrate / 1000) as _);
                        metadata.audio_codec_id =
                            audio_codec_name_to_id(&v.codec).map(|id| id as u32);
                        audio_var = Some(var);
                    }
                }
                _ => {}
            }
        }

        Ok(Self {
            state: ConnectionState::Disconnected,
            dest: dst.to_string(),
            session,
            metadata,
            video_variant: video_var.cloned().context("video stream missing")?,
            audio_variant: audio_var.cloned(),
            muxer: FlvMuxer::new(),
            avc_processor: Default::default(),
            aac_processor: Default::default(),
            out_tx: None,
            in_rx: None,
            video_timestamp: 0,
            audio_timestamp: 0,
        })
    }

    fn stream_key(&self) -> Result<StreamKey> {
        let url = Url::parse(&self.dest)?;
        let mut paths = url.path_segments().context("Invalid URL")?;
        let (key, app) = (
            paths.next_back().context("Missing stream key")?.to_string(),
            paths.next_back().context("Missing stream app")?.to_string(),
        );
        Ok(StreamKey { app, key })
    }

    pub async fn connect(&mut self) -> Result<()> {
        let mut hs = Handshake::new(PeerType::Server);

        let u = Url::parse(&self.dest)?;
        let addr = u.socket_addrs(|| Some(1935))?;
        let mut socket = TcpStream::connect(addr.first().context("DNS resolution failed")?).await?;
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

                    let q = self.session.handle_input(&remaining_bytes)?;

                    let (tx, rx) = Self::spawn_socket_io(socket);
                    for packet in q {
                        tx.send(packet)?;
                    }

                    self.state = ConnectionState::Connecting;
                    self.out_tx.replace(tx);
                    self.in_rx.replace(rx);
                    return Ok(());
                }
            }
        }
    }

    fn spawn_socket_io(
        socket: TcpStream,
    ) -> (
        UnboundedSender<ClientSessionResult>,
        UnboundedReceiver<Vec<u8>>,
    ) {
        let (in_tx, in_rx) = unbounded_channel();
        let (out_tx, mut out_rx) = unbounded_channel();

        // socket IO task
        tokio::spawn(async move {
            let mut buf = [0; 4096];
            let (mut s_rx, mut s_tx) = socket.into_split();
            loop {
                tokio::select! {
                    Ok(rlen) = s_rx.read(&mut buf) => {
                        if  rlen == 0 {
                            info!("EOF reached while reading");
                            break;
                        }
                        if let Err(e) = in_tx.send(buf[..rlen].to_vec()) {
                            error!("Error sending data to channel {}", e);
                        }
                    }
                    Some(packet) = out_rx.recv() => {
                        match packet {
                            ClientSessionResult::OutboundResponse(r) => {
                                if let Err(e) = s_tx.write_all(&r.bytes).await {
                                    error!("Failed to send outbound response: {:?}", e);
                                    break;
                                }
                            }
                            p => warn!("Unexpected packet: {:?}", p),
                        }
                    }
                }
            }
        });
        (out_tx, in_rx)
    }

    fn handle_session_result(
        &mut self,
        result: Result<ClientSessionResult, ClientSessionError>,
    ) -> Result<()> {
        match result {
            Ok(data) => {
                if let Some(tx) = self.out_tx.as_ref() {
                    tx.send(data)?;
                } else {
                    bail!(
                        "Cant send data in {} state, (tx channel not setup)",
                        self.state
                    )
                }
            }
            Err(e) => {
                return Err(anyhow!("Failed to publish video data {}", e));
            }
        }
        Ok(())
    }

    fn write_flv_tag(
        &mut self,
        tag_type: u8,
        timestamp: u32,
        data: Bytes,
    ) -> Result<BytesMut, FlvMuxerError> {
        let body_len = data.len();
        self.muxer
            .write_flv_tag_header(tag_type, body_len as _, timestamp)?;
        self.muxer.write_flv_tag_body(BytesMut::from(data))?;
        self.muxer.write_previous_tag_size((11 + body_len) as _)?;
        let data = self.muxer.writer.extract_current_bytes();
        Ok(data)
    }

    fn write_flv_header(&mut self, has_video: bool, has_audio: bool) -> Result<BytesMut> {
        self.muxer
            .write_flv_header(has_audio, has_video)
            .map_err(|e| anyhow!("failed to write flv header {}", e))?;
        self.muxer
            .write_previous_tag_size(0)
            .map_err(|e| anyhow!("failed to write flv header {}", e))?;

        Ok(self.muxer.writer.extract_current_bytes())
    }

    unsafe fn send_packet(&mut self, variant: &Uuid, packet: *mut AVPacket) -> Result<()> {
        if packet.is_null() || (*packet).size == 0 {
            return Ok(());
        }
        let data = std::slice::from_raw_parts((*packet).data, (*packet).size as _);
        let timestamp_ms = ((*packet).pts as f64 * av_q2d((*packet).time_base) * 1000.0).round() as u32;

        if *variant == self.video_variant.id() {
            // let bytes = BytesMut::from(data);
            // let data = self
            //     .avc_processor
            //     .nalus_to_mpeg4avc(vec![bytes])
            //     .map_err(|e| anyhow!(e))?;
            let data = self
                .write_flv_tag(9, timestamp_ms, Bytes::from(data))
                .map_err(|e| anyhow!("Muxer failed: {}", e))?;
            let res = self.session.publish_video_data(
                Bytes::from(data),
                RtmpTimestamp::new(timestamp_ms),
                false,
            );
            self.handle_session_result(res)?;
        } else if Some(*variant) == self.audio_variant.as_ref().map(|v| v.id()) {
            let data = self
                .write_flv_tag(8, timestamp_ms, Bytes::from(data))
                .map_err(|e| anyhow!("Muxer failed: {}", e))?;
            let res = self.session.publish_audio_data(
                Bytes::from(data),
                RtmpTimestamp::new(timestamp_ms),
                false,
            );
            self.handle_session_result(res)?;
        } else {
            // ignored
        }

        Ok(())
    }

    fn read_drain(&mut self) -> Result<()> {
        if let Some(rx) = self.in_rx.as_mut() {
            while let Ok(data) = rx.try_recv() {
                let res = self.session.handle_input(&data)?;
                if let Some(tx) = self.out_tx.as_ref() {
                    for tres in res {
                        match tres {
                            ClientSessionResult::RaisedEvent(e) => match e {
                                ClientSessionEvent::ConnectionRequestAccepted => {
                                    self.state = ConnectionState::Connected;
                                    info!("Connection request accepted");
                                }
                                ClientSessionEvent::ConnectionRequestRejected { description } => {
                                    error!("Connection request rejected: {}", description);
                                    self.state = ConnectionState::Disconnected;
                                }
                                ClientSessionEvent::PublishRequestAccepted => {
                                    self.state = ConnectionState::PublishingMetadata;
                                    info!("Publish request accepted");
                                }
                                _ => trace!("Unexpected event type {:?}", e),
                            },
                            ClientSessionResult::OutboundResponse(_) => {
                                tx.send(tres)?;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Egress for RtmpEgress {
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        loop {
            self.read_drain()?;
            match self.state {
                ConnectionState::Connecting => {
                    let k = self.stream_key()?;
                    let data = self.session.request_connection(k.app)?;
                    self.out_tx
                        .as_ref()
                        .context("TX channel missing")?
                        .send(data)?;
                    self.state = ConnectionState::RequestedConnection;
                }
                ConnectionState::Connected => {
                    let k = self.stream_key()?;
                    let data = self
                        .session
                        .request_publishing(k.key, PublishRequestType::Live)?;
                    self.out_tx
                        .as_ref()
                        .context("TX channel missing")?
                        .send(data)?;
                    self.state = ConnectionState::RequestedPublish;
                }
                ConnectionState::PublishingMetadata => {
                    let data = self.session.publish_metadata(&self.metadata)?;
                    self.out_tx
                        .as_ref()
                        .context("TX channel missing")?
                        .send(data)?;

                    // let data = self.write_flv_header(true, self.audio_variant.is_some())?;
                    // let res = self.session.publish_video_data(
                    //     data.freeze(),
                    //     RtmpTimestamp::new(0),
                    //     false,
                    // )?;
                    // self.out_tx
                    //     .as_ref()
                    //     .context("TX channel missing")?
                    //     .send(res)?;

                    self.state = ConnectionState::Publishing;
                }
                ConnectionState::Disconnected => {
                    // nothing, (yet)
                    break;
                }
                ConnectionState::Publishing => {
                    self.send_packet(variant, packet)?;
                    break;
                }
                _ => {
                    //loop
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        Ok(EgressResult::None)
    }

    unsafe fn reset(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
