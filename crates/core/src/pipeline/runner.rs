use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::mem::transmute;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::egress::hls::HlsEgress;
use crate::egress::recorder::RecorderEgress;
use crate::egress::{Egress, EgressResult};
use crate::generator::FrameGenerator;
use crate::ingress::ConnectionInfo;
use crate::mux::SegmentType;
use crate::overseer::{IngressInfo, IngressStream, IngressStreamType, Overseer};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::variant::{StreamMapping, VariantStream};
use anyhow::{anyhow, bail, Context, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_WEBP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_clone, av_frame_free, av_get_sample_fmt, av_packet_free, av_rescale_q, AVFrame,
    AVPacket, AV_NOPTS_VALUE,
};
use ffmpeg_rs_raw::{
    cstr, get_frame_from_hw, AudioFifo, Decoder, Demuxer, Encoder, Resample, Scaler, StreamType,
};
use log::{debug, error, info, warn};
use tokio::runtime::Handle;
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
        gen: FrameGenerator,
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

    /// Simple mapping to copy streams
    copy_stream: HashMap<Uuid, Uuid>,

    /// All configured egress'
    egress: Vec<Box<dyn Egress>>,

    /// Overseer managing this pipeline
    overseer: Arc<dyn Overseer>,

    fps_counter_start: Instant,
    fps_last_frame_ctr: u64,

    /// Total number of frames produced
    frame_ctr: u64,

    /// Output directory where all stream data is saved
    out_dir: String,

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
    cmd_channel: Option<Receiver<PipelineCommand>>,
}

unsafe impl Send for PipelineRunner {}

impl PipelineRunner {
    pub fn new(
        handle: Handle,
        out_dir: String,
        overseer: Arc<dyn Overseer>,
        connection: ConnectionInfo,
        recv: Box<dyn Read + Send>,
        url: Option<String>,
        command: Option<Receiver<PipelineCommand>>,
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
            copy_stream: Default::default(),
            fps_counter_start: Instant::now(),
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

    /// Save a decoded frame as a thumbnail
    unsafe fn generate_thumb_from_frame(&mut self, frame: *mut AVFrame) -> Result<()> {
        if self.thumb_interval > 0 && (self.frame_ctr % self.thumb_interval) == 0 {
            let frame = av_frame_clone(frame).addr();
            let dir = PathBuf::from(&self.out_dir).join(self.connection.id.to_string());
            if !dir.exists() {
                std::fs::create_dir_all(&dir)?;
            }
            std::thread::spawn(move || unsafe {
                let mut frame = frame as *mut AVFrame; //TODO: danger??
                let thumb_start = Instant::now();

                let dst_pic = dir.join("thumb.webp");
                if let Err(e) = Self::save_thumb(frame, &dst_pic) {
                    warn!("Failed to save thumb: {}", e);
                }

                let thumb_duration = thumb_start.elapsed();
                av_frame_free(&mut frame);
                info!(
                    "Saved thumb ({}ms) to: {}",
                    thumb_duration.as_millis(),
                    dst_pic.display(),
                );
            });
        }
        Ok(())
    }

    /// Switch to idle mode with placeholder content generation
    unsafe fn switch_to_idle_mode(&mut self, config: &PipelineConfig) -> Result<()> {
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

        let mut gen = FrameGenerator::from_av_streams(
            video_stream as *const _,
            audio_stream.map(|s| s as *const _),
        )?;

        // Set starting PTS to continue from last frame
        gen.set_starting_pts(self.last_video_pts, self.last_audio_pts);
        self.state = RunnerState::Idle {
            start_time: Instant::now(),
            gen,
        };

        self.consecutive_decode_failures = 0; // Reset counter when entering idle mode
        info!("Switched to idle mode - generating placeholder content");
        Ok(())
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

    /// Process frame in normal mode (live stream)
    unsafe fn process_normal_mode(&mut self, config: &PipelineConfig) -> Result<Vec<EgressResult>> {
        let (mut pkt, _stream) = self.demuxer.get_packet()?;
        if pkt.is_null() {
            warn!("Demuxer get_packet failed, entering idle mode");
            self.switch_to_idle_mode(config)
                .context("Failed to switch to idle mode after demuxer failure")?;
            Ok(vec![])
        } else {
            let res = self.process_packet(pkt)?;
            av_packet_free(&mut pkt);
            Ok(res)
        }
    }

    /// Process frame in idle mode (placeholder content)
    unsafe fn process_idle_mode(&mut self, config: &PipelineConfig) -> Result<Vec<EgressResult>> {
        // Check if idle timeout has been reached
        if let Some(duration) = self.state.idle_duration() {
            if duration > Duration::from_secs(IDLE_TIMEOUT_SECS) {
                info!(
                    "Idle timeout reached ({} seconds), ending stream",
                    IDLE_TIMEOUT_SECS
                );
                return Err(anyhow!("Idle timeout reached"));
            }
        }

        // Generate next frame from idle mode generator
        if let RunnerState::Idle {
            gen, start_time, ..
        } = &mut self.state
        {
            gen.begin()?;

            gen.fill_color([0, 0, 0, 255])?;
            let message = format!(
                "Stream Offline - {} seconds",
                start_time.elapsed().as_secs()
            );
            gen.write_text(&message, 48.0, 50.0, 50.0)?;
            gen.write_text("Please reconnect to resume streaming", 24.0, 50.0, 120.0)?;

            let frame = gen.next()?;
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

    unsafe fn process_packet(&mut self, packet: *mut AVPacket) -> Result<Vec<EgressResult>> {
        let config = if let Some(config) = &self.config {
            config.clone()
        } else {
            bail!("Pipeline not configured, cant process packet")
        };

        // Process all packets (original or converted)
        let mut egress_results = vec![];
        // TODO: For copy streams, skip decoder
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
                    packet_info, e, self.consecutive_decode_failures, self.max_consecutive_failures
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

        Ok(egress_results)
    }

    /// process the frame in the pipeline
    unsafe fn process_frame(
        &mut self,
        config: &PipelineConfig,
        stream_index: usize,
        frame: *mut AVFrame,
    ) -> Result<Vec<EgressResult>> {
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
                warn!("Frame had nowhere to go in {} :/", var.id());
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

    unsafe fn encode_mux_frame(
        egress: &mut Vec<Box<dyn Egress>>,
        var: &VariantStream,
        encoder: &mut Encoder,
        frame: *mut AVFrame,
    ) -> Result<Vec<EgressResult>> {
        let mut ret = vec![];
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
        // pass new packets to egress
        for mut pkt in packets {
            for eg in egress.iter_mut() {
                let er = eg.process_pkt(pkt, &var.id())?;
                ret.push(er);
            }
            av_packet_free(&mut pkt);
        }

        Ok(ret)
    }

    /// EOF, cleanup
    unsafe fn flush(&mut self) -> Result<()> {
        if self.config.is_some() {
            self.handle.block_on(async {
                if let Err(e) = self.overseer.on_end(&self.connection.id).await {
                    error!("Failed to end stream: {e}");
                }
            });
        }
        for (var, enc) in &mut self.encoders {
            for mut pkt in enc.encode_frame(ptr::null_mut())? {
                for eg in self.egress.iter_mut() {
                    eg.process_pkt(pkt, var)?;
                }
                av_packet_free(&mut pkt);
            }
        }
        for eg in self.egress.iter_mut() {
            eg.reset()?;
        }
        Ok(())
    }

    pub fn run(&mut self) {
        loop {
            unsafe {
                match self.once() {
                    Ok(c) => {
                        if !c {
                            // let drop handle flush
                            break;
                        }
                    }
                    Err(e) => {
                        // let drop handle flush
                        error!("Pipeline run failed: {}", e);
                        break;
                    }
                }
            }
        }
    }

    fn handle_command(&mut self) -> Result<Option<bool>> {
        if let Some(cmd) = &self.cmd_channel {
            while let Ok(c) = cmd.try_recv() {
                match c {
                    PipelineCommand::Shutdown => {
                        self.state = RunnerState::Shutdown;
                        return Ok(Some(true));
                    }
                    _ => warn!("Unexpected command: {:?}", c),
                }
            }
        }
        Ok(None)
    }

    unsafe fn once(&mut self) -> Result<bool> {
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
                    if let EgressResult::Segments { created, deleted } = er {
                        if let Err(e) = self
                            .overseer
                            .on_segments(&self.connection.id, &created, &deleted)
                            .await
                        {
                            bail!("Failed to process segment {}", e.to_string());
                        }
                    }
                }
                Ok(())
            })?;
        }
        let elapsed = Instant::now().sub(self.fps_counter_start).as_secs_f32();
        if elapsed >= 2f32 {
            let n_frames = self.frame_ctr - self.fps_last_frame_ctr;
            debug!("Average fps: {:.2}", n_frames as f32 / elapsed);
            self.fps_counter_start = Instant::now();
            self.fps_last_frame_ctr = self.frame_ctr;
        }
        Ok(true)
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
        let cfg = self
            .handle
            .block_on(async { self.overseer.start_stream(&self.connection, &i_info).await })?;

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
        // setup scaler/encoders
        for out_stream in &cfg.variants {
            match out_stream {
                VariantStream::Video(v) => {
                    self.encoders.insert(out_stream.id(), v.try_into()?);
                    self.scalers.insert(out_stream.id(), Scaler::new());
                }
                VariantStream::Audio(a) => {
                    let enc = a.try_into()?;
                    let fmt = unsafe { av_get_sample_fmt(cstr!(a.sample_fmt.as_str())) };
                    let rs = Resample::new(fmt, a.sample_rate as _, a.channels as _);
                    let f = AudioFifo::new(fmt, a.channels as _)?;
                    self.resampler.insert(out_stream.id(), (rs, f));
                    self.encoders.insert(out_stream.id(), enc);
                }
                _ => continue,
            }
        }

        // TODO: Setup copy streams

        // Setup egress
        for e in &cfg.egress {
            let c = e.config();
            let encoders = self.encoders.iter().filter_map(|(k, v)| {
                if c.variants.contains(k) {
                    let var = cfg.variants.iter().find(|x| x.id() == *k)?;
                    Some((var, v))
                } else {
                    None
                }
            });
            match e {
                EgressType::HLS(_) => {
                    let hls = HlsEgress::new(
                        &self.connection.id,
                        &self.out_dir,
                        encoders,
                        SegmentType::MPEGTS,
                    )?;
                    self.egress.push(Box::new(hls));
                }
                EgressType::Recorder(_) => {
                    let rec = RecorderEgress::new(&self.connection.id, &self.out_dir, encoders)?;
                    self.egress.push(Box::new(rec));
                }
                _ => warn!("{} is not implemented", e),
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
            self.copy_stream.clear();
            self.egress.clear();

            info!(
                "PipelineRunner cleaned up resources for stream: {}",
                self.connection.id
            );
        }
    }
}
