use std::collections::{HashMap, HashSet};
use std::io::{Read, stdout};
use std::mem::transmute;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::egress::hls::HlsEgress;
use crate::egress::recorder::RecorderEgress;
#[cfg(feature = "egress-rtmp")]
use crate::egress::rtmp::RtmpEgress;
use crate::egress::{Egress, EgressResult, EncoderOrSourceStream};
use crate::generator::FrameGenerator;
use crate::ingress::{ConnectionInfo, EndpointStats};
use crate::overseer::{IngressInfo, IngressStream, IngressStreamType, Overseer, StatsType};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{Context, Result, anyhow, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_WEBP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AV_NOPTS_VALUE, AVFrame, AVPacket, av_frame_clone, av_frame_free, av_get_sample_fmt,
    av_packet_clone, av_packet_copy_props, av_packet_free, av_rescale_q,
};
use ffmpeg_rs_raw::{
    AudioFifo, Decoder, Demuxer, Encoder, Resample, Scaler, StreamType, cstr, get_frame_from_hw,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, error, info, trace, warn};
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{
    EnvFilter, Layer, fmt, layer::SubscriberExt,
};
use uuid::Uuid;

/// Idle mode timeout in seconds
const IDLE_TIMEOUT_SECS: u64 = 60;

/// Circuit breaker threshold for consecutive decode failures
const DEFAULT_MAX_CONSECUTIVE_FAILURES: u32 = 50;

/// Runner state for handling normal vs idle modes
pub enum RunnerState {
    /// Normal operation - processing live stream
    Normal,
    /// Idle mode - generating placeholder content after disconnection
    Idle {
        start_time: Instant,
        frame_gen: FrameGenerator,
    },
    /// Pipeline should shut down and do any cleanup
    Shutdown,
}

impl RunnerState {
    /// Check if currently in idle mode
    pub fn is_idle(&self) -> bool {
        matches!(self, RunnerState::Idle { .. })
    }

    /// Get idle duration, returns None if not in idle mode
    pub fn idle_duration(&self) -> Option<Duration> {
        match self {
            RunnerState::Idle { start_time, .. } => Some(start_time.elapsed()),
            _ => None,
        }
    }
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

    /// Scaler for a variant (variant_id, Scaler)
    scalers: HashMap<Uuid, Scaler>,

    /// Resampler for a variant (variant_id, Resample+FIFO)
    resampler: HashMap<Uuid, (Resample, AudioFifo)>,

    /// Encoder for a variant (variant_id, Encoder)
    encoders: HashMap<Uuid, Encoder>,

    /// All configured egress'
    egress: Vec<Box<dyn Egress>>,

    /// Overseer managing this pipeline
    overseer: Arc<dyn Overseer>,

    stats_start: Instant,
    fps_last_frame_ctr: u64,

    /// Total number of frames produced
    frame_ctr: u64,

    /// Output directory where all stream data is saved
    out_dir: PathBuf,

    /// Thumbnail generation interval (0 = disabled)
    thumb_interval: u64,

    /// Current runner state (normal or idle)
    state: RunnerState,

    /// Counter for consecutive decode failures
    consecutive_decode_failures: u32,

    /// Maximum consecutive failures before triggering circuit breaker
    max_consecutive_failures: u32,

    /// Last video PTS for continuity in idle mode
    last_video_pts: i64,

    /// Last audio PTS for continuity in idle mode
    last_audio_pts: i64,

    /// Command receiver for external process control
    cmd_channel: Option<UnboundedReceiver<PipelineCommand>>,
}

unsafe impl Send for PipelineRunner {}

impl PipelineRunner {
    pub fn new(
        handle: Handle,
        out_dir: PathBuf,
        overseer: Arc<dyn Overseer>,
        connection: ConnectionInfo,
        recv: Box<dyn Read + Send>,
        url: Option<String>,
        command: Option<UnboundedReceiver<PipelineCommand>>,
    ) -> Result<Self> {
        Ok(Self {
            handle,
            out_dir,
            overseer,
            connection,
            config: Default::default(),
            demuxer: Demuxer::new_custom_io(recv, url)?,
            decoder: Decoder::new(),
            scalers: Default::default(),
            resampler: Default::default(),
            encoders: Default::default(),
            stats_start: Instant::now(),
            egress: Vec::new(),
            frame_ctr: 0,
            fps_last_frame_ctr: 0,
            thumb_interval: 1800,
            state: RunnerState::Normal,
            consecutive_decode_failures: 0,
            max_consecutive_failures: DEFAULT_MAX_CONSECUTIVE_FAILURES,
            last_video_pts: 0,
            last_audio_pts: 0,
            cmd_channel: command,
        })
    }

    pub fn set_demuxer_buffer_size(&mut self, buffer_size: usize) {
        self.demuxer.set_buffer_size(buffer_size);
    }

    pub fn set_demuxer_format(&mut self, format: &str) {
        self.demuxer.set_format(format);
    }

    /// Save image to disk
    unsafe fn save_thumb(frame: *mut AVFrame, dst_pic: &Path) -> Result<()> {
        unsafe {
            let mut free_frame = false;
            // use scaler to convert pixel format if not YUV420P
            let mut frame = if (*frame).format != transmute(AV_PIX_FMT_YUV420P) {
                let mut sw = Scaler::new();
                let new_frame = sw.process_frame(
                    frame,
                    (*frame).width as _,
                    (*frame).height as _,
                    AV_PIX_FMT_YUV420P,
                )?;
                free_frame = true;
                new_frame
            } else {
                frame
            };

            let encoder = Encoder::new(AV_CODEC_ID_WEBP)?
                .with_height((*frame).height)
                .with_width((*frame).width)
                .with_pix_fmt(transmute((*frame).format))
                .open(None)?;

            encoder.save_picture(frame, dst_pic.to_str().unwrap())?;
            if free_frame {
                av_frame_free(&mut frame);
            }
            Ok(())
        }
    }

    /// Save a decoded frame as a thumbnail
    fn generate_thumb_from_frame(&mut self, frame: *mut AVFrame) -> Result<()> {
        if self.thumb_interval > 0 && (self.frame_ctr % self.thumb_interval) == 0 {
            let frame = unsafe { av_frame_clone(frame).addr() };
            let dst_pic = self.out_dir.join("thumb.webp");
            let overseer = self.overseer.clone();
            let handle = self.handle.clone();
            let id = self.connection.id;
            std::thread::spawn(move || unsafe {
                let mut frame = frame as *mut AVFrame; //TODO: danger??
                let thumb_start = Instant::now();

                if let Err(e) = Self::save_thumb(frame, &dst_pic) {
                    av_frame_free(&mut frame);
                    warn!("Failed to save thumb: {}", e);
                }

                let thumb_duration = thumb_start.elapsed();
                info!(
                    "Saved thumb ({}ms) to: {}",
                    thumb_duration.as_millis(),
                    dst_pic.display(),
                );
                if let Err(e) = handle.block_on(overseer.on_thumbnail(
                    &id,
                    (*frame).width as _,
                    (*frame).height as _,
                    &dst_pic,
                )) {
                    warn!("Failed to handle on_thumbnail: {}", e);
                }
                av_frame_free(&mut frame);
            });
        }
        Ok(())
    }

    /// Switch to idle mode with placeholder content generation
    unsafe fn switch_to_idle_mode(&mut self, config: &PipelineConfig) -> Result<()> {
        unsafe {
            if self.state.is_idle() {
                return Ok(()); // Already in idle mode
            }

            // Get streams directly from demuxer for correct timebase and properties
            let video_stream = self.demuxer.get_stream(config.video_src)?;
            let audio_stream = if let Some(audio_src) = config.audio_src {
                Some(self.demuxer.get_stream(audio_src)?)
            } else {
                None
            };

            let mut frame_gen = FrameGenerator::from_av_streams(
                video_stream as *const _,
                audio_stream.map(|s| s as *const _),
            )?;

            // Set starting PTS to continue from last frame
            frame_gen.set_starting_pts(self.last_video_pts, self.last_audio_pts);
            self.state = RunnerState::Idle {
                start_time: Instant::now(),
                frame_gen,
            };

            self.consecutive_decode_failures = 0; // Reset counter when entering idle mode
            info!("Switched to idle mode - generating placeholder content");
            Ok(())
        }
    }

    /// Check if circuit breaker should trigger due to consecutive failures
    fn should_trigger_circuit_breaker(&self) -> bool {
        self.consecutive_decode_failures >= self.max_consecutive_failures
    }

    /// Handle decode failure with circuit breaker logic
    unsafe fn handle_decode_failure(
        &mut self,
        config: &PipelineConfig,
    ) -> Result<Vec<EgressResult>> {
        unsafe {
            if self.should_trigger_circuit_breaker() {
                error!(
                    "Circuit breaker triggered: {} consecutive decode failures exceeded threshold of {}. Switching to idle mode.",
                    self.consecutive_decode_failures, self.max_consecutive_failures
                );

                self.switch_to_idle_mode(config)
                    .context("Circuit breaker triggered but unable to switch to idle mode")?;
            }

            // Return empty result to skip this packet
            Ok(vec![])
        }
    }

    /// Process frame in normal mode (live stream)
    unsafe fn process_normal_mode(&mut self, config: &PipelineConfig) -> Result<Vec<EgressResult>> {
        unsafe {
            let (pkt, _stream) = self.demuxer.get_packet()?;
            if pkt.is_null() {
                warn!("Demuxer get_packet failed, entering idle mode");
                self.switch_to_idle_mode(config)
                    .context("Failed to switch to idle mode after demuxer failure")?;
                Ok(vec![])
            } else {
                self.process_packet(pkt)
            }
        }
    }

    /// Process frame in idle mode (placeholder content)
    unsafe fn process_idle_mode(&mut self, config: &PipelineConfig) -> Result<Vec<EgressResult>> {
        unsafe {
            // Check if idle timeout has been reached
            if let Some(duration) = self.state.idle_duration()
                && duration > Duration::from_secs(IDLE_TIMEOUT_SECS)
            {
                info!(
                    "Idle timeout reached ({} seconds), ending stream",
                    IDLE_TIMEOUT_SECS
                );
                return Err(anyhow!("Idle timeout reached"));
            }

            // Generate next frame from idle mode generator
            if let RunnerState::Idle {
                frame_gen,
                start_time,
                ..
            } = &mut self.state
            {
                frame_gen.begin()?;

                frame_gen.fill_color([0, 0, 0, 255])?;
                let message = format!(
                    "Stream Offline - {} seconds",
                    start_time.elapsed().as_secs()
                );
                frame_gen.write_text(&message, 48.0, 50.0, 50.0)?;
                frame_gen.write_text("Please reconnect to resume streaming", 24.0, 50.0, 120.0)?;

                let frame = frame_gen.next()?;
                let stream = if (*frame).sample_rate > 0 {
                    // Audio frame
                    config
                        .audio_src
                        .context("got audio frame with no audio src?")?
                } else {
                    // Video frame
                    config.video_src
                };
                self.process_frame(config, stream, frame)
            } else {
                bail!("process_idle_mode called but not in idle state")
            }
        }
    }

    unsafe fn process_packet(&mut self, mut packet: *mut AVPacket) -> Result<Vec<EgressResult>> {
        unsafe {
            let config = if let Some(config) = &self.config {
                config.clone()
            } else {
                bail!("Pipeline not configured, cant process packet")
            };

            // Process all packets (original or converted)
            let mut egress_results = vec![];
            // only process via decoder if there is more than 1 encoder
            if !self.encoders.is_empty() {
                let frames = match self.decoder.decode_pkt(packet) {
                    Ok(f) => {
                        // Reset failure counter on successful decode
                        self.consecutive_decode_failures = 0;
                        f
                    }
                    Err(e) => {
                        self.consecutive_decode_failures += 1;

                        // Enhanced error logging with context
                        let packet_info = if !packet.is_null() {
                            format!(
                                "stream_idx={}, size={}, pts={}, dts={}",
                                (*packet).stream_index,
                                (*packet).size,
                                (*packet).pts,
                                (*packet).dts
                            )
                        } else {
                            "null packet".to_string()
                        };

                        warn!(
                            "Error decoding packet ({}): {}. Consecutive failures: {}/{}. Skipping packet.",
                            packet_info,
                            e,
                            self.consecutive_decode_failures,
                            self.max_consecutive_failures
                        );

                        return self.handle_decode_failure(&config);
                    }
                };

                for (frame, stream_idx) in frames {
                    let stream = self.demuxer.get_stream(stream_idx as usize)?;
                    // Adjust frame pts time without start_offset
                    // Egress streams don't have a start time offset
                    if !stream.is_null() {
                        if (*stream).start_time != AV_NOPTS_VALUE {
                            (*frame).pts -= (*stream).start_time;
                        }
                        (*frame).time_base = (*stream).time_base;
                    }

                    let results = self.process_frame(&config, stream_idx as usize, frame)?;
                    egress_results.extend(results);
                }
            }

            // egress (mux) copy variants
            for var in config.variants {
                match var {
                    VariantStream::CopyAudio(v) if v.src_index() == (*packet).stream_index as _ => {
                        egress_results.extend(Self::egress_packet(
                            &mut self.egress,
                            packet,
                            &v.id(),
                        )?);
                    }
                    VariantStream::CopyVideo(v) if v.src_index() == (*packet).stream_index as _ => {
                        // count frames for copy only pipelines
                        if self.encoders.is_empty() {
                            self.frame_ctr += 1;
                        }
                        egress_results.extend(Self::egress_packet(
                            &mut self.egress,
                            packet,
                            &v.id(),
                        )?);
                    }
                    _ => {}
                }
            }

            av_packet_free(&mut packet);
            Ok(egress_results)
        }
    }

    /// process the frame in the pipeline
    unsafe fn process_frame(
        &mut self,
        config: &PipelineConfig,
        stream_index: usize,
        frame: *mut AVFrame,
    ) -> Result<Vec<EgressResult>> {
        unsafe {
            // Copy frame from GPU if using hwaccel decoding
            let mut frame = get_frame_from_hw(frame)?;

            let mut egress_results = Vec::new();
            // Get the variants which want this pkt
            let pkt_vars = config
                .variants
                .iter()
                .filter(|v| v.src_index() == stream_index);
            for var in pkt_vars {
                let enc = if let Some(enc) = self.encoders.get_mut(&var.id()) {
                    enc
                } else {
                    continue;
                };

                // scaling / resampling
                let mut new_frame = false;
                match var {
                    VariantStream::Video(v) => {
                        let mut frame = if let Some(s) = self.scalers.get_mut(&v.id()) {
                            new_frame = true;
                            s.process_frame(frame, v.width, v.height, transmute(v.pixel_format))?
                        } else {
                            frame
                        };
                        egress_results.extend(Self::encode_mux_frame(
                            &mut self.egress,
                            var,
                            enc,
                            frame,
                        )?);
                        if new_frame {
                            av_frame_free(&mut frame);
                        }
                    }
                    VariantStream::Audio(a) => {
                        if let Some((r, f)) = self.resampler.get_mut(&a.id()) {
                            let frame_size = (*enc.codec_context()).frame_size;
                            let mut resampled_frame = r.process_frame(frame)?;
                            f.buffer_frame(resampled_frame)?;
                            av_frame_free(&mut resampled_frame);
                            // drain FIFO
                            while let Some(mut frame) = f.get_frame(frame_size as usize)? {
                                // Set correct timebase for audio (1/sample_rate)
                                (*frame).time_base.num = 1;
                                (*frame).time_base.den = a.sample_rate as i32;

                                egress_results.extend(Self::encode_mux_frame(
                                    &mut self.egress,
                                    var,
                                    enc,
                                    frame,
                                )?);
                                av_frame_free(&mut frame);
                            }
                        } else {
                            egress_results.extend(Self::encode_mux_frame(
                                &mut self.egress,
                                var,
                                enc,
                                frame,
                            )?);
                        }
                    }
                    _ => {}
                }
            }

            // Track last PTS values for continuity in idle mode
            if stream_index == config.video_src {
                self.last_video_pts = (*frame).pts + (*frame).duration;
                self.generate_thumb_from_frame(frame)?;
                self.frame_ctr += 1;
            } else if Some(stream_index) == config.audio_src {
                self.last_audio_pts = (*frame).pts + (*frame).duration;
            }

            av_frame_free(&mut frame);
            Ok(egress_results)
        }
    }

    unsafe fn encode_mux_frame(
        egress: &mut Vec<Box<dyn Egress>>,
        var: &VariantStream,
        encoder: &mut Encoder,
        frame: *mut AVFrame,
    ) -> Result<Vec<EgressResult>> {
        unsafe {
            // before encoding frame, rescale timestamps
            if !frame.is_null() {
                let enc_ctx = encoder.codec_context();
                (*frame).pict_type = AV_PICTURE_TYPE_NONE;
                (*frame).pts = av_rescale_q((*frame).pts, (*frame).time_base, (*enc_ctx).time_base);
                (*frame).pkt_dts =
                    av_rescale_q((*frame).pkt_dts, (*frame).time_base, (*enc_ctx).time_base);
                (*frame).duration =
                    av_rescale_q((*frame).duration, (*frame).time_base, (*enc_ctx).time_base);
                (*frame).time_base = (*enc_ctx).time_base;
            }

            let packets = encoder.encode_frame(frame)?;
            let mut ret = vec![];
            for mut pkt in packets {
                ret.extend(Self::egress_packet(egress, pkt, &var.id())?);
                av_packet_free(&mut pkt);
            }
            Ok(ret)
        }
    }

    unsafe fn egress_packet(
        egress: &mut Vec<Box<dyn Egress>>,
        pkt: *mut AVPacket,
        variant: &Uuid,
    ) -> Result<Vec<EgressResult>> {
        unsafe {
            let mut ret = vec![];
            for eg in egress.iter_mut() {
                // packet needs to be cloned because AVFormat output context always consumes the packet
                // if we have more than 1 egress it should be cloned
                let mut pkt_clone = av_packet_clone(pkt);
                av_packet_copy_props(pkt_clone, pkt);
                trace!(
                    "EGRESS PKT: var={}, idx={}, pts={}, dur={}",
                    variant,
                    (*pkt_clone).stream_index,
                    (*pkt_clone).pts,
                    (*pkt_clone).duration
                );
                let er = eg.process_pkt(pkt_clone, variant)?;
                ret.push(er);
                av_packet_free(&mut pkt_clone);
            }
            Ok(ret)
        }
    }

    /// EOF, cleanup
    unsafe fn flush(&mut self) -> Result<()> {
        unsafe {
            for (var, enc) in &mut self.encoders {
                for mut pkt in enc.encode_frame(ptr::null_mut())? {
                    for eg in self.egress.iter_mut() {
                        eg.process_pkt(pkt, var)?;
                    }
                    av_packet_free(&mut pkt);
                }
            }

            // Reset egress handlers and collect deleted segments
            let mut reset_results = Vec::new();
            for eg in self.egress.iter_mut() {
                let result = eg.reset()?;
                reset_results.push(result);
            }

            // Process reset results and notify overseer of deleted segments
            if self.config.is_some() {
                self.handle.block_on(async {
                    for result in reset_results {
                        if let EgressResult::Segments { created, deleted } = result
                            && !deleted.is_empty()
                            && let Err(e) = self
                                .overseer
                                .on_segments(&self.connection.id, &created, &deleted)
                                .await
                        {
                            error!("Failed to notify overseer of deleted segments: {e}");
                        }
                    }
                    if let Err(e) = self.overseer.on_end(&self.connection.id).await {
                        error!("Failed to end stream: {e}");
                    }
                });
            }

            Ok(())
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
                    .with_filter(EnvFilter::new("zap_stream_core=debug,zap_stream=debug")),
            );

        tracing::subscriber::with_default(logger, || {
            info!("Pipeline run starting");
            loop {
                unsafe {
                    match self.once() {
                        Ok(c) => {
                            if !c {
                                // let drop handle flush
                                info!("Pipeline run ending normally");
                                break;
                            }
                        }
                        Err(e) => {
                            // let drop handle flush
                            error!(error = %e, "Pipeline run failed");
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
                        self.state = RunnerState::Shutdown;
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

    unsafe fn once(&mut self) -> Result<bool> {
        unsafe {
            if let Some(r) = self.handle_command()? {
                return Ok(r);
            }
            self.setup()?;

            let config = if let Some(config) = &self.config {
                config.clone()
            } else {
                bail!("Pipeline not configured, cannot run")
            };

            // run transcoder pipeline
            let results = match &mut self.state {
                RunnerState::Normal => self.process_normal_mode(&config)?,
                RunnerState::Idle { .. } => self.process_idle_mode(&config)?,
                _ => return Ok(false), // skip once, nothing to do
            };

            // egress results - process async operations without blocking if possible
            if !results.is_empty() {
                self.handle.block_on(async {
                    for er in results {
                        if let EgressResult::Segments { created, deleted } = er
                            && let Err(e) = self
                                .overseer
                                .on_segments(&self.connection.id, &created, &deleted)
                                .await
                        {
                            bail!("Failed to process segment {}", e.to_string());
                        }
                    }
                    Ok(())
                })?;
            }
            let elapsed = Instant::now().sub(self.stats_start).as_secs_f32();
            if elapsed >= 2f32 {
                let n_frames = self.frame_ctr - self.fps_last_frame_ctr;
                let avg_fps = n_frames as f32 / elapsed;
                debug!("Average fps: {:.2}", avg_fps);
                self.stats_start = Instant::now();
                self.fps_last_frame_ctr = self.frame_ctr;

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

        let info = unsafe {
            self.demuxer
                .probe_input()
                .map_err(|e| anyhow!("Demuxer probe failed: {}", e))?
        };
        info!("{}", info);
        // convert to internal type
        let i_info = IngressInfo {
            bitrate: info.bitrate,
            streams: info
                .streams
                .iter()
                .map(|s| IngressStream {
                    index: s.index,
                    stream_type: match s.stream_type {
                        StreamType::Video => IngressStreamType::Video,
                        StreamType::Audio => IngressStreamType::Audio,
                        StreamType::Subtitle => IngressStreamType::Subtitle,
                    },
                    codec: s.codec,
                    format: s.format,
                    width: s.width,
                    height: s.height,
                    fps: s.fps,
                    sample_rate: s.sample_rate,
                    channels: s.channels,
                    language: s.language.clone(),
                })
                .collect(),
        };
        let cfg = self.handle.block_on(async {
            self.overseer
                .start_stream(&mut self.connection, &i_info)
                .await
        })?;

        let inputs: HashSet<usize> = cfg.variants.iter().map(|e| e.src_index()).collect();
        self.decoder.enable_hw_decoder_any();
        for input_idx in inputs {
            let stream = info.streams.iter().find(|f| f.index == input_idx).unwrap();
            self.decoder.setup_decoder(stream, None)?;
        }
        self.setup_encoders(&cfg)?;
        info!("{}", cfg);
        self.config = Some(cfg);
        Ok(())
    }

    fn setup_encoders(&mut self, cfg: &PipelineConfig) -> Result<()> {
        // Analyze egress types to determine which encoders need GLOBAL_HEADER
        let mut encoders_need_global_header = HashSet::new();

        for egress in &cfg.egress {
            let needs_global_header = match egress {
                // HLS fMP4 mode needs GLOBAL_HEADER
                EgressType::HLS(_, _, crate::mux::SegmentType::FMP4) => true,
                // HLS MPEGTS mode doesn't need GLOBAL_HEADER
                EgressType::HLS(_, _, crate::mux::SegmentType::MPEGTS) => false,
                // Recorder (MP4) needs GLOBAL_HEADER
                EgressType::Recorder(_) => true,
                // RTMP forwarder typically doesn't need GLOBAL_HEADER
                EgressType::RTMPForwarder(_, _) => false,
            };

            if needs_global_header {
                for variant_id in egress.variants() {
                    encoders_need_global_header.insert(*variant_id);
                }
            }
        }

        // setup scaler/encoders
        for out_stream in &cfg.variants {
            let need_global_header = encoders_need_global_header.contains(&out_stream.id());
            match out_stream {
                VariantStream::Video(v) => {
                    let encoder = v.create_encoder(need_global_header)?;
                    self.encoders.insert(out_stream.id(), encoder);
                    self.scalers.insert(out_stream.id(), Scaler::new());
                }
                VariantStream::Audio(a) => {
                    let enc = a.create_encoder(need_global_header)?;
                    let fmt = unsafe { av_get_sample_fmt(cstr!(a.sample_fmt.as_str())) };
                    let rs = Resample::new(fmt, a.sample_rate as _, a.channels as _);
                    let f = AudioFifo::new(fmt, a.channels as _)?;
                    self.resampler.insert(out_stream.id(), (rs, f));
                    self.encoders.insert(out_stream.id(), enc);
                }
                _ => continue,
            }
        }

        // Setup egress
        for e in &cfg.egress {
            let c = e.variants();
            let vars = c
                .iter()
                .map_while(|x| cfg.variants.iter().find(|z| z.id() == *x));
            let variant_mapping = vars.map_while(|v| {
                if let Some(e) = self.encoders.get(&v.id()) {
                    Some((v, EncoderOrSourceStream::Encoder(e)))
                } else {
                    Some((
                        v,
                        EncoderOrSourceStream::SourceStream(unsafe {
                            self.demuxer.get_stream(v.src_index()).ok()?
                        }),
                    ))
                }
            });
            match e {
                EgressType::HLS(_, len, seg) => {
                    let hls = HlsEgress::new(self.out_dir.clone(), variant_mapping, *seg, *len)?;
                    self.egress.push(Box::new(hls));
                }
                EgressType::Recorder(_) => {
                    let rec = RecorderEgress::new(self.out_dir.clone(), variant_mapping)?;
                    self.egress.push(Box::new(rec));
                }
                #[cfg(feature = "egress-rtmp")]
                EgressType::RTMPForwarder(_, dst) => {
                    let mut fwd = RtmpEgress::new(dst, variant_mapping)?;
                    if let Err(e) = self.handle.block_on(async { fwd.connect().await }) {
                        error!("Failed to connect forwarder: {}", e);
                    } else {
                        self.egress.push(Box::new(fwd));
                    }
                }
                _ => bail!("Unhandled egress type: {:?}", e),
            }
        }
        Ok(())
    }
}

impl Drop for PipelineRunner {
    fn drop(&mut self) {
        unsafe {
            // First try to flush properly
            if let Err(e) = self.flush() {
                error!("Failed to flush pipeline during drop: {}", e);
            }

            // Clear all collections to ensure proper Drop cleanup
            // The FFmpeg objects should implement Drop properly in ffmpeg-rs-raw
            self.encoders.clear();
            self.scalers.clear();
            self.resampler.clear();
            self.egress.clear();

            info!(
                "PipelineRunner cleaned up resources for stream: {}",
                self.connection.id
            );
        }
    }
}
