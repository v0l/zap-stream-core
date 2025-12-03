use crate::egress::{Egress, EgressEncoderConfig, EgressResult, EncoderOrSourceStream};
use crate::metrics::PacketMetrics;
use crate::overseer::IngressStream;
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{Context, Result, anyhow, bail};
use bytes::{BufMut, Bytes, BytesMut};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AVPacket, av_packet_clone, av_packet_copy_props, av_q2d,
};
use rml_rtmp::chunk_io::Packet;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionError, ClientSessionEvent,
    ClientSessionResult, PublishRequestType, StreamMetadata,
};
use rml_rtmp::time::RtmpTimestamp;
use std::collections::VecDeque;
use std::fmt::Display;
use std::io::Write;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{error, info, trace, warn};
use url::Url;
use uuid::Uuid;
use xflv::errors::FlvMuxerError;
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

struct QueuedPacket {
    variant: Uuid,
    packet: *mut AVPacket,
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

    out_tx: Option<UnboundedSender<ClientSessionResult>>,
    in_rx: Option<UnboundedReceiver<Vec<u8>>>,

    // Monotonic timestamp tracking
    video_pts: i64,
    audio_pts: i64,

    // packets which are held until the connection is ready
    pkt_queue: VecDeque<QueuedPacket>,

    flv_dump: std::fs::File,

    // Packet metrics tracking
    metrics: PacketMetrics,
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
            out_tx: None,
            in_rx: None,
            video_pts: 0,
            audio_pts: 0,
            pkt_queue: VecDeque::new(),
            flv_dump: std::fs::File::create("./dump.flv")?,
            metrics: PacketMetrics::new("RTMP Egress", None),
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

    /// Check if packet contains SPS/PPS sequence headers
    fn is_sequence_header(data: &[u8]) -> bool {
        // Look for SPS (NAL type 7) or PPS (NAL type 8) in the packet
        let mut i = 0;
        while i < data.len() {
            // Look for start codes: 0x00 0x00 0x00 0x01 or 0x00 0x00 0x01
            if i + 3 < data.len()
                && data[i] == 0x00
                && data[i + 1] == 0x00
                && data[i + 2] == 0x00
                && data[i + 3] == 0x01
            {
                // 4-byte start code
                if i + 4 < data.len() {
                    let nal_type = data[i + 4] & 0x1F;
                    if nal_type == 7 || nal_type == 8 {
                        // SPS or PPS
                        return true;
                    }
                }
                i += 4;
            } else if i + 2 < data.len()
                && data[i] == 0x00
                && data[i + 1] == 0x00
                && data[i + 2] == 0x01
            {
                // 3-byte start code
                if i + 3 < data.len() {
                    let nal_type = data[i + 3] & 0x1F;
                    if nal_type == 7 || nal_type == 8 {
                        // SPS or PPS
                        return true;
                    }
                }
                i += 3;
            } else {
                i += 1;
            }
        }
        false
    }

    /// Check if audio packet contains AAC sequence header
    fn is_aac_sequence_header(data: &[u8]) -> bool {
        // AAC sequence headers are typically very small (usually 2-5 bytes)
        // and contain AudioSpecificConfig data
        // For now, we'll use a simple heuristic: very small packets are likely sequence headers
        // A more robust implementation would parse the AudioSpecificConfig structure
        data.len() <= 16 && data.len() >= 2
    }

    unsafe fn send_packet(&mut self, variant: &Uuid, packet: *mut AVPacket) -> Result<()> {
        if packet.is_null() || (*packet).size == 0 {
            return Ok(());
        }
        let data = std::slice::from_raw_parts((*packet).data, (*packet).size as _);

        // Update metrics with packet data (auto-reports when interval elapsed)
        self.metrics.update((*packet).size as usize);

        if *variant == self.video_variant.id() {
            // TODO: figure out why encoded video frames have no duration
            let duration_ms = (av_q2d((*packet).time_base) * 1000.0).round() as i64; // 1 frame
            // Create proper FLV video data format
            let mut video_data = BytesMut::new();

            // VideoTagHeader: FrameType (4 bits) + CodecID (4 bits)
            // FrameType: 1 = keyframe, 2 = inter frame
            // CodecID: 7 = AVC (H.264)
            let frame_type =
                if (*packet).flags & ffmpeg_rs_raw::ffmpeg_sys_the_third::AV_PKT_FLAG_KEY != 0 {
                    1
                } else {
                    2
                };
            let video_tag_header = (frame_type << 4) | 7; // CodecID = 7 for AVC
            video_data.put_u8(video_tag_header);

            // AVCPacketType: 0 = sequence header (SPS/PPS), 1 = AVC NALU, 2 = end of sequence
            let avc_packet_type = if Self::is_sequence_header(data) {
                0 // Sequence header for SPS/PPS
            } else {
                1 // Regular NAL units
            };
            video_data.put_u8(avc_packet_type);

            // CompositionTime: 24-bit signed offset (PTS - DTS)
            // For sequence headers, composition time should be 0
            let composition_time = if avc_packet_type == 0 {
                0
            } else {
                self.video_pts
            };
            video_data.put_u8(((composition_time >> 16) & 0xFF) as u8);
            video_data.put_u8(((composition_time >> 8) & 0xFF) as u8);
            video_data.put_u8((composition_time & 0xFF) as u8);

            // Video data (raw H.264 NAL units)
            video_data.extend_from_slice(data);

            // Use write_flv_tag to create proper FLV tag structure
            let flv_tag = self
                .write_flv_tag(9, self.video_pts as u32, video_data.freeze())
                .map_err(|e| anyhow!("Failed to write FLV video tag: {}", e))?;

            self.flv_dump.write_all(flv_tag.as_ref())?;
            let res = self.session.publish_video_data(
                flv_tag.freeze(),
                RtmpTimestamp::new(self.video_pts as u32), // Use DTS for RTMP timestamp
                false,
            );
            self.handle_session_result(res)?;
            self.video_pts += duration_ms;
        } else if Some(*variant) == self.audio_variant.as_ref().map(|v| v.id()) {
            let duration_ms =
                (av_q2d((*packet).time_base) * 1000.0 * (*packet).duration as f64).round() as i64;
            // Create proper FLV audio data format
            let mut audio_data = BytesMut::new();

            // AudioTagHeader: SoundFormat (4 bits) + SoundRate (2 bits) + SoundSize (1 bit) + SoundType (1 bit)
            // SoundFormat: 10 = AAC
            // SoundRate: 3 = 44kHz (we'll use this as default)
            // SoundSize: 1 = 16-bit
            // SoundType: 1 = stereo
            let audio_tag_header = (10 << 4) | (3 << 2) | (1 << 1) | 1;
            audio_data.put_u8(audio_tag_header);

            // AACPacketType: 0 = sequence header (AudioSpecificConfig), 1 = AAC raw
            let aac_packet_type = if Self::is_aac_sequence_header(data) {
                0 // Sequence header for AudioSpecificConfig
            } else {
                1 // Regular AAC frames
            };
            audio_data.put_u8(aac_packet_type);

            // Audio data (raw AAC frames or AudioSpecificConfig)
            audio_data.extend_from_slice(data);

            // Use write_flv_tag to create proper FLV tag structure
            let flv_tag = self
                .write_flv_tag(8, self.audio_pts as _, audio_data.freeze())
                .map_err(|e| anyhow!("Failed to write FLV audio tag: {}", e))?;

            self.flv_dump.write_all(flv_tag.as_ref())?;
            let res = self.session.publish_audio_data(
                flv_tag.freeze(),
                RtmpTimestamp::new(self.audio_pts as _),
                false,
            );
            self.handle_session_result(res)?;
            self.audio_pts += duration_ms;
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

    fn queue_packet(&mut self, packet: *mut AVPacket, variant: &Uuid) -> Result<()> {
        // clone the packet again so that it's not freed later
        let pkt = unsafe {
            let np = av_packet_clone(packet);
            if np.is_null() {
                bail!("Failed to clone packet");
            }
            let ret = av_packet_copy_props(np, packet);
            if ret != 0 {
                bail!("Failed to copy packet props");
            }
            np
        };
        self.pkt_queue.push_back(QueuedPacket {
            variant: *variant,
            packet: pkt,
        });
        Ok(())
    }
}

impl Egress for RtmpEgress {
    unsafe fn process_pkt(
        &mut self,
        packet: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<EgressResult> {
        // skip packet if not forwarding
        if *variant != self.video_variant.id()
            && self
                .audio_variant
                .as_ref()
                .map(|v| v.id() != *variant)
                .unwrap_or(true)
        {
            return Ok(EgressResult::None);
        }
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
                    self.queue_packet(packet, variant)?;
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
                    self.queue_packet(packet, variant)?;
                }
                ConnectionState::PublishingMetadata => {
                    let data = self.session.publish_metadata(&self.metadata)?;
                    self.out_tx
                        .as_ref()
                        .context("TX channel missing")?
                        .send(data)?;

                    // Write FLV header at start of stream
                    let flv_tag = self.write_flv_header(true, self.audio_variant.is_some())?;
                    self.flv_dump.write_all(flv_tag.as_ref())?;

                    self.state = ConnectionState::Publishing;
                    self.queue_packet(packet, variant)?;
                }
                ConnectionState::Disconnected => {
                    // nothing, (yet)
                    self.queue_packet(packet, variant)?;
                    break;
                }
                ConnectionState::Publishing => {
                    // push first the queued packets
                    if self.pkt_queue.len() > 0 {
                        let pkts: Vec<QueuedPacket> = self.pkt_queue.drain(..).collect();
                        info!("Sending {} queued packets", pkts.len());
                        for pkt in pkts {
                            self.send_packet(&pkt.variant, pkt.packet)?;
                        }
                    }

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

    unsafe fn reset(&mut self) -> anyhow::Result<EgressResult> {
        Ok(EgressResult::None)
    }
}
