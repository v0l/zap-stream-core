use std::collections::{HashMap, HashSet};
use std::io::{Read, stdout};
use std::ops::Sub;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "egress-hls")]
use crate::egress::hls::HlsEgress;
#[cfg(feature = "egress-moq")]
use crate::egress::moq::MoqEgress;
use crate::egress::muxer_egress::MuxerEgress;
use crate::egress::{Egress, EncoderOrSourceStream, EncoderVariant, EncoderVariantGroup};
use crate::ingress::{ConnectionInfo, EndpointStats};
use crate::overseer::{IngressInfo, IngressStream, Overseer, StatsType};
use crate::pipeline::worker::{PipelineWorkerThreadBuilder, WorkerThreadCommand};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::reorder::FrameReorderBuffer;
use crate::variant::VariantStream;
use anyhow::{Result, anyhow, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AV_NOPTS_VALUE, AV_PKT_FLAG_KEY};
use ffmpeg_rs_raw::{AvFrameRef, AvPacketRef, Decoder, Demuxer, Muxer, StreamType, get_frame_from_hw, get_frame_from_hw_with_fmt};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, error, info, trace, warn};
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt};
use uuid::Uuid;

/// Runner state for handling normal vs idle modes
pub enum RunnerState {
    /// Normal operation - processing live stream
    Normal,
    /// Pipeline should shut down and do any cleanup
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// External process requested clean shutdown
    Shutdown,
    /// Metrics provided by the ingress
    IngressMetrics(EndpointStats),
    /// Metrics provided by egress components
    EgressMetrics(EndpointStats),
}

#[derive(Debug, Clone)]
pub struct PipelineStats {
    pub average_fps: f32,
    pub total_frames: u64,
    /// If pipeline is in normal running state
    pub is_running: bool,
}

/// Pipeline runner is the main entry process for stream transcoding
///
/// Each client connection spawns a new [PipelineRunner] and it should be run in its own thread
/// using [crate::ingress::spawn_pipeline]
pub struct PipelineRunner {
    /// Async runtime handle
    handle: Handle,

    /// Input stream connection info
    pub connection: ConnectionInfo,

    /// Configuration for this pipeline (variants, egress config etc.)
    config: Option<PipelineConfig>,

    /// Where the pipeline gets packets from
    demuxer: Demuxer,
    /// Singleton decoder for all stream
    decoder: Decoder,

    /// Channels for sending work to each thread by variant id
    worker_channels: HashMap<Uuid, Sender<WorkerThreadCommand>>,

    /// All configured egress'
    egress: Arc<Mutex<Vec<Box<dyn Egress>>>>,
    /// Overseer managing this pipeline
    overseer: Arc<dyn Overseer>,

    last_stats: Instant,
    fps_last_frame_ctr: u64,

    /// Total number of frames produced
    frame_ctr: u64,
    /// Output directory where all stream data is saved
    out_dir: PathBuf,
    /// Thumbnail generation interval in milliseconds (0 = disabled)
    thumb_interval: u64,
    /// Time when the last thumbnail was generated
    last_thumb: Instant,
    /// Current runner state (normal or idle)
    state: RunnerState,
    /// Last video PTS for continuity in idle mode
    last_video_pts: i64,
    /// Last audio PTS for continuity in idle mode
    last_audio_pts: i64,
    /// Command receiver for external process control
    cmd_channel: Option<UnboundedReceiver<PipelineCommand>>,

    /// Frame reorder buffers for video source streams (to ensure frames go to encoder in PTS order)
    /// Key is the source stream index
    frame_reorder_buffers: HashMap<usize, FrameReorderBuffer<AvFrameRef>>,

    /// PTS offset per source stream to fix duplicate PTS values
    /// Key is the stream index, value is (last_pts, offset)
    pts_offsets: HashMap<usize, (i64, i64)>,
}

unsafe impl Send for PipelineRunner {}

impl PipelineRunner {
    pub const RECORDING_PATH: &'static str = "recording.mp4";

    pub fn new(
        handle: Handle,
        out_dir: PathBuf,
        overseer: Arc<dyn Overseer>,
        connection: ConnectionInfo,
        recv: Box<dyn Read + Send>,
        url: Option<String>,
        command: Option<UnboundedReceiver<PipelineCommand>>,
    ) -> Result<Self> {
        const DEFAULT_THUMB_INTERVAL: u64 = 1000 * 60 * 5;
        Ok(Self {
            handle,
            out_dir,
            overseer,
            connection,
            config: Default::default(),
            demuxer: Demuxer::new_custom_io(recv, url)?,
            decoder: Decoder::new(),
            worker_channels: Default::default(),
            last_stats: Instant::now(),
            egress: Default::default(),
            frame_ctr: 0,
            fps_last_frame_ctr: 0,
            thumb_interval: DEFAULT_THUMB_INTERVAL,
            last_thumb: Instant::now().sub(Duration::from_millis(DEFAULT_THUMB_INTERVAL)),
            state: RunnerState::Normal,
            last_video_pts: 0,
            last_audio_pts: 0,
            cmd_channel: command,
            frame_reorder_buffers: Default::default(),
            pts_offsets: Default::default(),
        })
    }

    pub fn set_demuxer_buffer_size(&mut self, buffer_size: usize) {
        self.demuxer.set_buffer_size(buffer_size);
    }

    pub fn set_demuxer_format(&mut self, format: &str) {
        self.demuxer.set_format(format);
    }

    pub fn is_transcoding(&self) -> bool {
        if let Some(ref c) = self.config {
            c.variants.iter().any(|v| match v {
                VariantStream::Video(_) => true,
                _ => false,
            })
        } else {
            false
        }
    }

    /// Save a decoded frame as a thumbnail
    fn generate_thumb_from_frame(&mut self, frame: &AvFrameRef) -> Result<()> {
        if self.thumb_interval > 0
            && self.last_thumb.elapsed().as_millis() > self.thumb_interval as u128
        {
            self.last_thumb = Instant::now();
            let frame = frame.clone();
            let dst_pic = self.out_dir.join("thumb.webp");

            // send thumbnail on the first worker
            // TODO: pick one that has less work to do (copy streams / audio)
            if let Some(w) = self.worker_channels.values().next() {
                if let Err(e) = w.send(WorkerThreadCommand::SaveThumbnail {
                    frame,
                    dst: dst_pic,
                }) {
                    error!("Error sending worker thread command: {}", e);
                }
            }
        }
        Ok(())
    }

    /// Process frame in normal mode (live stream)
    unsafe fn process_normal_mode(&mut self) -> Result<()> {
        unsafe {
            let (pkt, _stream) = self.demuxer.get_packet()?;

            if let Some(pkt) = pkt {
                self.process_packet(pkt)
            } else {
                // EOF, exit
                self.flush();
                Ok(())
            }
        }
    }

    /// Fix duplicate/backwards PTS values on frames after reorder buffer
    /// Frames come out of the reorder buffer in PTS order, so we track last_pts
    fn mangle_frame_pts(&mut self, stream_idx: usize, frame: &mut AvFrameRef) {
        let (last_pts, offset) = self.pts_offsets.entry(stream_idx).or_insert((i64::MIN, 0));

        if frame.pts != AV_NOPTS_VALUE {
            // Apply existing offset first
            let adjusted_pts = frame.pts + *offset;

            // If adjusted PTS is still <= last PTS, we have a discontinuity
            if *last_pts != i64::MIN && adjusted_pts <= *last_pts {
                // Calculate additional offset needed: jump past last_pts
                let additional_offset = *last_pts + 1 - adjusted_pts;
                *offset += additional_offset;
                warn!(
                    "PTS fix: stream={}, original_pts={}, last_pts={}, new_offset={}, delta={}",
                    stream_idx, frame.pts, *last_pts, *offset, additional_offset
                );
            }

            // Apply total offset to PTS
            frame.pts += *offset;

            // Update last_pts for this stream (with offset applied)
            *last_pts = frame.pts;
        }
    }

    unsafe fn process_packet(&mut self, packet: AvPacketRef) -> Result<()> {
        let Some(config) = self.config.as_ref().map(|c| c.clone()) else {
            bail!("Pipeline not configured, cant process packet")
        };

        // Track last PTS values for continuity in idle mode
        let stream_index = packet.stream_index as usize;
        if stream_index == config.video_src {
            self.last_video_pts = packet.pts + packet.duration;
            self.frame_ctr += 1;
        } else if Some(stream_index) == config.audio_src {
            self.last_audio_pts = packet.pts + packet.duration;
        }

        // Check if this packet needs transcoding (not just copying)
        let needs_transcode = config.is_transcoding_src(stream_index);

        // Check if we need to decode for thumbnail generation
        // Only decode for thumbnails if it's the video source, thumbnails are enabled,
        // and the packet is a keyframe (contains a full frame of data)
        let is_keyframe = packet.flags & AV_PKT_FLAG_KEY != 0;
        let needs_thumb_decode = stream_index == config.video_src
            && self.thumb_interval > 0
            && self.last_thumb.elapsed().as_millis() > self.thumb_interval as u128
            && is_keyframe;

        // only process via decoder if we have encoders and (need transcoding OR thumbnail generation)
        if config.is_transcoding() && (needs_transcode || needs_thumb_decode) {
            trace!(
                "PKT->DECODER: stream={}, pts={}, dts={}, duration={}, flags={}",
                stream_index, packet.pts, packet.dts, packet.duration, packet.flags
            );
            self.transcode_pkt(Some(&packet))?;
        }

        // egress (mux) copy variants
        for var in config.variants {
            match var {
                VariantStream::CopyAudio(v) if v.src_index == stream_index => {
                    self.send_work(
                        v.id,
                        WorkerThreadCommand::MuxPacket {
                            packet: packet.clone(),
                        },
                    )?;
                }
                VariantStream::CopyVideo(v) if v.src_index == stream_index => {
                    self.send_work(
                        v.id,
                        WorkerThreadCommand::MuxPacket {
                            packet: packet.clone(),
                        },
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn send_work(&self, variant: Uuid, job: WorkerThreadCommand) -> Result<()> {
        let Some(q) = self.worker_channels.get(&variant) else {
            bail!("No worker channel setup for variant: {}", variant);
        };

        q.send(job)
            .map_err(|e| anyhow!("Error sending work: {}", e))?;
        Ok(())
    }

    fn transcode_pkt(&mut self, pkt: Option<&AvPacketRef>) -> Result<()> {
        unsafe {
            let video_src = if let Some(ref config) = self.config {
                config.video_src as i32
            } else {
                bail!("Pipeline not configured, cant process packet")
            };
            let frames = self.decoder.decode_pkt(pkt)?;
            for (mut frame, stream_idx) in frames {
                trace!(
                    "DECODER->FRAME: stream={}, pts={}, pkt_dts={}, duration={}, tb={}/{}",
                    stream_idx,
                    frame.pts,
                    frame.pkt_dts,
                    frame.duration,
                    frame.time_base.num,
                    frame.time_base.den
                );
                let stream = self.demuxer.get_stream(stream_idx as usize)?;
                // Adjust frame pts time without start_offset
                // Egress streams don't have a start time offset
                if !stream.is_null() {
                    let start_time = (*stream).start_time;
                    if start_time != AV_NOPTS_VALUE && start_time != 0 {
                        frame.pts -= start_time;
                        trace!(
                            "FRAME PTS ADJUST: pts now={}, subtracted start_time={}",
                            frame.pts, start_time
                        );
                    }
                    frame.time_base = (*stream).time_base;
                }

                // Copy frame from GPU if using hwaccel decoding
                // TODO: add automatic pixel conversion in encoder step
                let frame = get_frame_from_hw_with_fmt(frame, AV_PIX_FMT_YUV420P)?;

                if stream_idx == video_src {
                    self.generate_thumb_from_frame(&frame)?;
                }

                self.process_frame(stream_idx as usize, frame)?;
            }
            Ok(())
        }
    }

    /// process the frame in the pipeline
    unsafe fn process_frame(&mut self, stream_index: usize, frame: AvFrameRef) -> Result<()> {
        // Check if this is a video or audio frame
        let is_video_frame = frame.width > 0 && frame.height > 0;

        let frames = if is_video_frame {
            trace!(
                "FRAME->REORDER: src={}, pts={}, pkt_dts={}, duration={}, tb={}/{}",
                stream_index,
                frame.pts,
                frame.pkt_dts,
                frame.duration,
                frame.time_base.num,
                frame.time_base.den
            );

            // Get or create reorder buffer for this source stream
            let reorder_buffer = self
                .frame_reorder_buffers
                .entry(stream_index)
                .or_insert_with(FrameReorderBuffer::new);

            // Push frame into reorder buffer and get any frames ready to encode
            let mut frames_to_encode = reorder_buffer.push(frame.pts, frame.duration, frame);

            // Process each reordered frame through all video variants
            for mut frame_ref in &mut frames_to_encode {
                // Fix PTS discontinuities after reorder (frames now in PTS order)
                self.mangle_frame_pts(stream_index, &mut frame_ref);

                trace!(
                    "REORDER->VARIANTS: src={}, pts={}, pkt_dts={}, duration={}, tb={}/{}",
                    stream_index,
                    frame_ref.pts,
                    frame_ref.pkt_dts,
                    frame_ref.duration,
                    frame_ref.time_base.num,
                    frame_ref.time_base.den
                );
            }
            frames_to_encode
        } else {
            vec![frame]
        };
        let Some(config) = &self.config else {
            bail!("Pipeline not configured, cant process frame")
        };
        for frame in frames {
            for var in config.variants.iter().filter(|v| {
                v.src_index() == stream_index
                    && match v {
                        VariantStream::Audio(_) | VariantStream::Video(_) => true,
                        _ => false,
                    }
            }) {
                self.send_work(
                    var.id(),
                    WorkerThreadCommand::EncodeFrame {
                        frame: frame.clone(),
                    },
                )?;
            }
        }

        Ok(())
    }

    /// EOF, cleanup
    fn flush(&mut self) {
        self.state = RunnerState::Shutdown;
        for (_, w) in self.worker_channels.drain() {
            if let Err(e) = w.send(WorkerThreadCommand::Flush) {
                warn!("Failed to send flush to worker thread: {}", e);
            }
        }
    }

    pub fn run(&mut self) {
        let file_appender = rolling::never(&self.out_dir, "pipeline.log");
        let (non_blocking, _guard) = non_blocking(file_appender);

        let logger = tracing_subscriber::registry()
            .with(
                fmt::Layer::new()
                    .with_writer(stdout)
                    .with_filter(EnvFilter::from_default_env()),
            )
            .with(
                fmt::Layer::new()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_thread_ids(true)
                    .with_filter(EnvFilter::new(
                        "zap_stream_core=debug,zap_stream=debug,ffmpeg=info,ffmpeg-rs-raw=info",
                    )),
            );

        tracing::subscriber::with_default(logger, || {
            info!("Pipeline run starting");
            loop {
                unsafe {
                    match self.once() {
                        Ok(c) => {
                            if !c {
                                // let drop handle flush
                                info!("Pipeline {} ending normally", self.connection.id);
                                break;
                            }
                        }
                        Err(e) => {
                            // let drop handle flush
                            error!(error = %e, "Pipeline {} run failed", self.connection.id);
                            break;
                        }
                    }
                }
            }
        });
    }

    fn handle_command(&mut self) -> Result<Option<bool>> {
        if let Some(cmd) = &mut self.cmd_channel {
            while let Ok(c) = cmd.try_recv() {
                match c {
                    PipelineCommand::Shutdown => {
                        self.flush();
                        return Ok(Some(true));
                    }
                    PipelineCommand::IngressMetrics(s) => {
                        let id = self.connection.id;
                        let overseer = self.overseer.clone();
                        self.handle.spawn(async move {
                            if let Err(e) = overseer.on_stats(&id, StatsType::Ingress(s)).await {
                                warn!("Pipeline stats error: {e}");
                            }
                        });
                    }
                    PipelineCommand::EgressMetrics(s) => {
                        let id = self.connection.id;
                        let overseer = self.overseer.clone();
                        self.handle.spawn(async move {
                            if let Err(e) = overseer.on_stats(&id, StatsType::Egress(s)).await {
                                warn!("Pipeline egress stats error: {e}");
                            }
                        });
                    }
                }
            }
        }
        Ok(None)
    }

    /// Run pipeline ones, false=exit
    unsafe fn once(&mut self) -> Result<bool> {
        unsafe {
            if let Some(r) = self.handle_command()? {
                return Ok(r);
            }
            self.setup()?;

            // run transcoder pipeline
            match &mut self.state {
                RunnerState::Normal => self.process_normal_mode()?,
                RunnerState::Shutdown => return Ok(false), // Shutdown requested
            }
            let elapsed = self.last_stats.elapsed().as_secs_f32();
            if elapsed >= 2.0 {
                self.last_stats = Instant::now();
                let n_frames = self.frame_ctr - self.fps_last_frame_ctr;
                let avg_fps = n_frames as f32 / elapsed;
                debug!("Average fps: {:.2}", avg_fps);
                self.fps_last_frame_ctr = self.frame_ctr;

                // Record playback rate metric
                if let Some(config) = &self.config {
                    let target_fps = config
                        .ingress_info
                        .streams
                        .get(config.video_src)
                        .map(|s| s.fps)
                        .unwrap_or(0.0);
                    let pipeline_id = self.connection.id.to_string();
                    crate::metrics::record_playback_rate(&pipeline_id, avg_fps, target_fps);
                }

                // emit metrics every 2s to overseer
                let overseer = self.overseer.clone();
                let metrics = PipelineStats {
                    average_fps: avg_fps,
                    total_frames: self.frame_ctr,
                    is_running: matches!(self.state, RunnerState::Normal),
                };
                let id = self.connection.id;
                self.handle.spawn(async move {
                    if let Err(e) = overseer.on_stats(&id, StatsType::Pipeline(metrics)).await {
                        warn!("Pipeline stats error: {e}");
                    }
                });
            }
            Ok(true)
        }
    }

    fn setup(&mut self) -> Result<()> {
        if self.config.is_some() {
            return Ok(());
        }

        //self.demuxer.enable_dts_monotonicity(0);
        let info = unsafe {
            self.demuxer
                .probe_input()
                .map_err(|e| anyhow!("Demuxer probe failed: {}", e))?
        };
        // convert to internal type
        let i_info = IngressInfo {
            bitrate: info.bitrate,
            streams: info
                .streams
                .iter()
                .filter_map(|s| {
                    Some(IngressStream {
                        index: s.index,
                        stream_type: match s.stream_type {
                            StreamType::Video => crate::overseer::StreamType::Video,
                            StreamType::Audio => crate::overseer::StreamType::Audio,
                            StreamType::Subtitle => crate::overseer::StreamType::Subtitle,
                            StreamType::Unknown => crate::overseer::StreamType::Unknown,
                        },
                        codec: s.codec,
                        format: s.format,
                        bitrate: s.bitrate,
                        profile: s.profile,
                        level: s.level,
                        color_range: s.color_range,
                        color_space: s.color_space,
                        width: s.width,
                        height: s.height,
                        fps: if s.fps.is_normal() { s.fps } else { 30.0 },
                        sample_rate: s.sample_rate,
                        channels: s.channels,
                        language: s.language.clone(),
                    })
                })
                .collect(),
        };
        let block_on_start = Instant::now();
        let cfg = self.handle.block_on(async {
            self.overseer
                .start_stream(&mut self.connection, &i_info)
                .await
        })?;
        crate::metrics::record_block_on_start_stream(block_on_start.elapsed());

        let inputs: HashSet<usize> = cfg.variants.iter().map(|e| e.src_index()).collect();
        self.decoder.enable_hw_decoder_any();
        for input_idx in inputs {
            let stream = info.streams.iter().find(|f| f.index == input_idx).unwrap();
            self.decoder.setup_decoder(stream, None)?;
        }
        self.setup_encoders(&cfg)?;
        info!("{}", cfg);
        // Log decoder info (including HW accel status)
        for input_idx in cfg.variants.iter().map(|e| e.src_index()).collect::<HashSet<_>>() {
            if let Some(dec) = self.decoder.get_decoder(input_idx as i32) {
                info!("Decoder for stream {}: {}", input_idx, dec.codec_name());
            }
        }
        self.config = Some(cfg);
        Ok(())
    }

    fn setup_encoders(&mut self, cfg: &PipelineConfig) -> Result<()> {
        // setup worker thread for each variant
        let mut workers = HashMap::new();
        for var in &cfg.variants {
            let w = PipelineWorkerThreadBuilder::try_from(var)?
                .with_egress(self.egress.clone())
                .with_handle(self.handle.clone())
                .with_overseer(self.overseer.clone())
                .with_pipeline_id(self.connection.id.clone());
            workers.insert(var.id(), w);
        }

        let get_source_stream = |v: &VariantStream| -> Option<EncoderOrSourceStream<'_>> {
            if let Some(e) = workers.get(&v.id()).and_then(|e| e.encoder()) {
                Some(EncoderOrSourceStream::Encoder(e))
            } else {
                Some(EncoderOrSourceStream::SourceStream(unsafe {
                    self.demuxer.get_stream(v.src_index()).ok()?
                }))
            }
        };

        let mut setup_egress: Vec<Box<dyn Egress>> = vec![];
        // Setup egress
        for e in &cfg.egress {
            let variant_mapping: Vec<_> = e
                .variants
                .iter()
                .filter_map(|v| {
                    let mut ret = EncoderVariantGroup {
                        id: v.id,
                        streams: vec![],
                    };
                    if let Some(v) = v.video {
                        let var = cfg.variants.iter().find(|x| x.id() == v)?;
                        let stream = get_source_stream(var)?;
                        ret.streams.push(EncoderVariant {
                            variant: var,
                            stream,
                        });
                    }
                    if let Some(a) = v.audio {
                        let var = cfg.variants.iter().find(|x| x.id() == a)?;
                        let stream = get_source_stream(var)?;
                        ret.streams.push(EncoderVariant {
                            variant: var,
                            stream,
                        });
                    }
                    if let Some(s) = v.subtitle {
                        let var = cfg.variants.iter().find(|x| x.id() == s)?;
                        let stream = get_source_stream(var)?;
                        ret.streams.push(EncoderVariant {
                            variant: var,
                            stream,
                        });
                    }
                    Some(ret)
                })
                .collect();
            match e.kind {
                #[cfg(feature = "egress-hls")]
                EgressType::HLS {
                    segment_type,
                    segment_length,
                    ..
                } => {
                    let hls = HlsEgress::new(
                        self.out_dir.clone(),
                        &variant_mapping,
                        segment_type,
                        segment_length,
                    )?;
                    setup_egress.push(Box::new(hls));
                }
                EgressType::Recorder { height, .. } => {
                    let match_var = variant_mapping.iter().find(|a| {
                        a.streams.iter().any(|b| match b.variant {
                            VariantStream::CopyVideo(v) | VariantStream::Video(v) => {
                                v.height == height
                            }
                            _ => false,
                        })
                    });
                    if let Some(a) = match_var {
                        let dst = self.out_dir.join(Self::RECORDING_PATH);
                        let muxer = unsafe {
                            Muxer::builder()
                                .with_output_path(dst.to_str().unwrap(), None)?
                                .build()?
                        };

                        let mut options = HashMap::new();
                        options.insert("movflags".to_string(), "faststart".to_string());
                        let rec = MuxerEgress::new("Recorder", muxer, a, Some(options))?;
                        setup_egress.push(Box::new(rec));
                    } else {
                        warn!(
                            "Could not find matching variant {}p, recording disabled!",
                            height
                        );
                    }
                }
                #[cfg(feature = "egress-rtmp")]
                EgressType::RTMPForwarder { ref destination, .. } => {
                    let Some(g) = variant_mapping.first() else {
                        warn!("Could not configure RTMP forwarder, no variants configured");
                        continue;
                    };
                    let muxer = unsafe {
                        let dest = destination.to_owned();
                        Muxer::builder()
                            .with_output_path(dest.as_str(), Some("flv"))?
                            .build()?
                    };
                    let fwd = MuxerEgress::new("RTMP Forward", muxer, g, None)?;
                    setup_egress.push(Box::new(fwd));
                }
                #[cfg(feature = "egress-moq")]
                EgressType::Moq { .. } => {
                    let block_on_start = Instant::now();
                    let origin = self
                        .handle
                        .block_on(async { self.overseer.get_moq_origin().await })?;
                    crate::metrics::record_block_on_moq_origin(block_on_start.elapsed());
                    let id = self.connection.id.to_string();
                    let moq = MoqEgress::new(self.handle.clone(), origin, &id, &variant_mapping)?;
                    setup_egress.push(Box::new(moq));
                }
                #[allow(unreachable_patterns)]
                _ => bail!("Unhandled egress type: {:?}", e),
            }
        }

        // setup channels to worker threads
        let mut w_run = vec![];
        for (k, w) in workers {
            let mut w = w.build()?;
            let Some(sender) = w.sender() else {
                bail!("Sender was already consumed, cannot continue!");
            };
            self.worker_channels.insert(k, sender);
            w_run.push(w);
        }
        // insert egress'
        {
            let mut el = self.egress.lock().expect("egress lock");
            for eg in setup_egress {
                el.push(eg);
            }
        }
        // run worker threads
        for w in w_run {
            w.run()?;
        }

        Ok(())
    }
}

impl Drop for PipelineRunner {
    fn drop(&mut self) {
        // First try to flush properly
        self.flush();

        info!(
            "PipelineRunner cleaned up resources for stream: {}",
            self.connection.id
        );
    }
}
