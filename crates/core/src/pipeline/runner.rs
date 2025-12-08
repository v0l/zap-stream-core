use std::collections::{HashMap, HashSet};
use std::io::{Read, stdout};
use std::mem::transmute;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "egress-hls")]
use crate::egress::hls::HlsEgress;
#[cfg(feature = "egress-moq")]
use crate::egress::moq::MoqEgress;
use crate::egress::recorder::RecorderEgress;
#[cfg(feature = "egress-rtmp")]
use crate::egress::rtmp::RtmpEgress;
use crate::egress::{
    Egress, EgressResult, EncoderOrSourceStream, EncoderVariant, EncoderVariantGroup,
};
use crate::ingress::{ConnectionInfo, EndpointStats};
use crate::overseer::{IngressInfo, IngressStream, IngressStreamType, Overseer, StatsType};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::reorder::FrameReorderBuffer;
use crate::variant::VariantStream;
use anyhow::{Result, anyhow, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_WEBP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    AV_NOPTS_VALUE, AV_PKT_FLAG_KEY, AVPixelFormat, av_rescale_q,
};
use ffmpeg_rs_raw::{
    AudioFifo, AvFrameRef, AvPacketRef, Decoder, Demuxer, Encoder, Resample, Scaler, StreamType,
    get_frame_from_hw,
};
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
            scalers: Default::default(),
            resampler: Default::default(),
            encoders: Default::default(),
            stats_start: Instant::now(),
            egress: Vec::new(),
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

    /// Save image to disk
    unsafe fn save_thumb(frame: &AvFrameRef, dst_pic: &Path) -> Result<()> {
        unsafe {
            let encoder = Encoder::new(AV_CODEC_ID_WEBP)?
                .with_height(frame.height)
                .with_width(frame.width)
                .with_pix_fmt(AV_PIX_FMT_YUV420P)
                .open(None)?;

            // use scaler to convert pixel format if not YUV420P
            if frame.format != transmute::<AVPixelFormat, i32>(AV_PIX_FMT_YUV420P) {
                let mut sw = Scaler::new();
                let new_frame = sw.process_frame(
                    frame,
                    frame.width as _,
                    frame.height as _,
                    AV_PIX_FMT_YUV420P,
                )?;
                encoder.save_picture(&new_frame, dst_pic.to_str().unwrap())?;
            } else {
                encoder.save_picture(frame, dst_pic.to_str().unwrap())?;
            };
            Ok(())
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
            let overseer = self.overseer.clone();
            let handle = self.handle.clone();
            let id = self.connection.id;
            std::thread::spawn(move || unsafe {
                let thumb_start = Instant::now();

                if let Err(e) = Self::save_thumb(&frame, &dst_pic) {
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
                    frame.width as _,
                    frame.height as _,
                    &dst_pic,
                )) {
                    warn!("Failed to handle on_thumbnail: {}", e);
                }
            });
        }
        Ok(())
    }

    /// Process frame in normal mode (live stream)
    unsafe fn process_normal_mode(&mut self) -> Result<Vec<EgressResult>> {
        unsafe {
            let (pkt, _stream) = self.demuxer.get_packet()?;

            if let Some(pkt) = pkt {
                self.process_packet(pkt)
            } else {
                // EOF, exit
                self.flush()
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

    unsafe fn process_packet(&mut self, packet: AvPacketRef) -> Result<Vec<EgressResult>> {
        unsafe {
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

            // Process all packets (original or converted)
            let mut egress_results = vec![];

            // Check if this packet needs transcoding (not just copying)
            let needs_transcode = config.variants.iter().any(|var| match var {
                VariantStream::Video(v) if v.src_index == stream_index => true,
                VariantStream::Audio(v) if v.src_index == stream_index => true,
                _ => false,
            });

            // Check if we need to decode for thumbnail generation
            // Only decode for thumbnails if it's the video source, thumbnails are enabled,
            // and the packet is a keyframe (contains a full frame of data)
            let is_keyframe = packet.flags & AV_PKT_FLAG_KEY != 0;
            let needs_thumb_decode = stream_index == config.video_src
                && self.thumb_interval > 0
                && self.last_thumb.elapsed().as_millis() > self.thumb_interval as u128
                && is_keyframe;

            // only process via decoder if we have encoders and (need transcoding OR thumbnail generation)
            if !self.encoders.is_empty() && (needs_transcode || needs_thumb_decode) {
                trace!(
                    "PKT->DECODER: stream={}, pts={}, dts={}, duration={}, flags={}",
                    stream_index, packet.pts, packet.dts, packet.duration, packet.flags
                );
                egress_results.extend(self.transcode_pkt(Some(&packet))?);
            }

            // egress (mux) copy variants
            for var in config.variants {
                match var {
                    VariantStream::CopyAudio(v) if v.src_index == stream_index => {
                        egress_results.extend(Self::egress_packet(
                            &mut self.egress,
                            &packet,
                            &v.id,
                        )?);
                    }
                    VariantStream::CopyVideo(v) if v.src_index == stream_index => {
                        egress_results.extend(Self::egress_packet(
                            &mut self.egress,
                            &packet,
                            &v.id,
                        )?);
                    }
                    _ => {}
                }
            }
            Ok(egress_results)
        }
    }

    fn transcode_pkt(&mut self, pkt: Option<&AvPacketRef>) -> Result<Vec<EgressResult>> {
        unsafe {
            let mut egress_results = vec![];
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
                let frame = get_frame_from_hw(frame)?;

                if stream_idx == video_src {
                    self.generate_thumb_from_frame(&frame)?;
                }

                let results = self.process_frame(stream_idx as usize, frame)?;
                egress_results.extend(results);
            }
            Ok(egress_results)
        }
    }

    /// process the frame in the pipeline
    unsafe fn process_frame(
        &mut self,
        stream_index: usize,
        frame: AvFrameRef,
    ) -> Result<Vec<EgressResult>> {
        unsafe {
            let mut egress_results = Vec::new();

            let Some(ref config) = self.config else {
                bail!("Pipeline not configured, cant process frame")
            };

            // Get the variants which want this frame (clone to avoid borrow issues)
            let pkt_vars: Vec<_> = config
                .variants
                .iter()
                .filter(|v| v.src_index() == stream_index)
                .cloned()
                .collect();

            // Check if this is a video or audio frame
            let is_video_frame = frame.width > 0 && frame.height > 0;

            if is_video_frame {
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
                let frames_to_encode = reorder_buffer.push(frame.pts, frame.duration, frame);

                // Process each reordered frame through all video variants
                for mut frame_ref in frames_to_encode {
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

                    // Process this reordered frame through all video variants
                    for var in &pkt_vars {
                        if let VariantStream::Video(v) = var {
                            let enc = self.encoders.get_mut(&v.id).unwrap();
                            if let Some(s) = self.scalers.get_mut(&v.id) {
                                trace!(
                                    "FRAME->SCALER: var={}, pts={}, pkt_dts={}, duration={}, tb={}/{}",
                                    v.id,
                                    frame_ref.pts,
                                    frame_ref.pkt_dts,
                                    frame_ref.duration,
                                    frame_ref.time_base.num,
                                    frame_ref.time_base.den
                                );

                                let Ok(pix_fmt) = v.pixel_format_id() else {
                                    warn!(
                                        "Could not scale frame without pixel_format for {}",
                                        v.id
                                    );
                                    continue;
                                };
                                let new_frame = s.process_frame(
                                    &frame_ref,
                                    v.width,
                                    v.height,
                                    transmute(pix_fmt),
                                )?;

                                egress_results.extend(Self::encode_mux_frame(
                                    &mut self.egress,
                                    var,
                                    enc,
                                    new_frame,
                                )?);
                            } else {
                                egress_results.extend(Self::encode_mux_frame(
                                    &mut self.egress,
                                    var,
                                    enc,
                                    frame_ref.clone(),
                                )?);
                            };
                        }
                    }
                }
            } else {
                // Process audio variants (no reordering needed)
                for var in &pkt_vars {
                    if let VariantStream::Audio(a) = var {
                        let var_id = var.id();
                        let enc = if let Some(enc) = self.encoders.get_mut(&var_id) {
                            enc
                        } else {
                            continue;
                        };

                        if let Some((r, f)) = self.resampler.get_mut(&a.id) {
                            let frame_size = (*enc.codec_context()).frame_size;
                            let resampled_frame = r.process_frame(&frame)?;
                            f.buffer_frame(&resampled_frame)?;
                            // drain FIFO
                            while let Some(mut audio_frame) = f.get_frame(frame_size as usize)? {
                                // Set correct timebase for audio (1/sample_rate)
                                audio_frame.time_base.num = 1;
                                audio_frame.time_base.den = a.sample_rate as i32;

                                // Need to re-borrow encoder after resampler borrow
                                let enc = self.encoders.get_mut(&var_id).unwrap();
                                egress_results.extend(Self::encode_mux_frame(
                                    &mut self.egress,
                                    var,
                                    enc,
                                    audio_frame,
                                )?);
                            }
                        } else {
                            egress_results.extend(Self::encode_mux_frame(
                                &mut self.egress,
                                var,
                                enc,
                                frame.clone(),
                            )?);
                        }
                    }
                }
            }

            // frame is dropped here (if not moved to reorder buffer), which calls av_frame_free
            Ok(egress_results)
        }
    }

    unsafe fn encode_mux_frame(
        egress: &mut Vec<Box<dyn Egress>>,
        var: &VariantStream,
        encoder: &mut Encoder,
        mut frame: AvFrameRef,
    ) -> Result<Vec<EgressResult>> {
        unsafe {
            // before encoding frame, rescale timestamps
            let enc_ctx = encoder.codec_context();
            frame.pict_type = AV_PICTURE_TYPE_NONE;
            frame.pts = av_rescale_q(frame.pts, frame.time_base, (*enc_ctx).time_base);
            frame.pkt_dts = AV_NOPTS_VALUE;
            frame.duration = av_rescale_q(frame.duration, frame.time_base, (*enc_ctx).time_base);
            frame.time_base = (*enc_ctx).time_base;

            trace!(
                "FRAME->ENCODER: var={}, pts={}, duration={}, tb={}/{}",
                var.id(),
                frame.pts,
                frame.duration,
                frame.time_base.num,
                frame.time_base.den
            );

            let packets = match encoder.encode_frame(Some(&frame)) {
                Ok(pkt) => pkt,
                Err(e) => {
                    error!(
                        "Failed to encode frame: var={}, pts={}, duration={} {}",
                        var.id(),
                        frame.pts,
                        frame.duration,
                        e
                    );
                    return Err(e);
                }
            };
            let mut ret = vec![];
            for pkt in packets {
                trace!(
                    "ENCODER->PKT: var={}, pts={}, dts={}, duration={}, flags={}",
                    var.id(),
                    pkt.pts,
                    pkt.dts,
                    pkt.duration,
                    pkt.flags
                );
                ret.extend(Self::egress_packet(egress, &pkt, &var.id())?);
            }
            Ok(ret)
        }
    }

    unsafe fn egress_packet(
        egress: &mut Vec<Box<dyn Egress>>,
        pkt: &AvPacketRef,
        variant: &Uuid,
    ) -> Result<Vec<EgressResult>> {
        let mut ret = vec![];
        for eg in egress.iter_mut() {
            trace!(
                "EGRESS PKT: var={}, idx={}, pts={}, dts={}, dur={}",
                variant, pkt.stream_index, pkt.pts, pkt.dts, pkt.duration
            );
            let er = eg.process_pkt(pkt.clone(), variant)?;
            ret.push(er);
        }
        Ok(ret)
    }

    /// EOF, cleanup
    unsafe fn flush(&mut self) -> Result<Vec<EgressResult>> {
        self.state = RunnerState::Shutdown;
        let mut reset_results = Vec::new();

        // flush decoder
        reset_results.extend(self.transcode_pkt(None)?);

        // flush encoders
        for (var, enc) in &mut self.encoders {
            for pkt in enc.encode_frame(None)? {
                for eg in self.egress.iter_mut() {
                    reset_results.push(eg.process_pkt(pkt.clone(), var)?);
                }
            }
        }

        // Reset egress handlers and collect deleted segments
        for eg in self.egress.iter_mut() {
            let result = eg.reset()?;
            reset_results.push(result);
        }

        reset_results.push(EgressResult::Flush);
        Ok(reset_results)
    }

    fn handle_egress_results(&self, results: Vec<EgressResult>) {
        // Process reset results and notify overseer of deleted segments
        if self.config.is_some() && !results.is_empty() {
            self.handle.block_on(async {
                for result in results {
                    match result {
                        EgressResult::Flush => {
                            if let Err(e) = self.overseer.on_end(&self.connection.id).await {
                                error!("Failed to end stream: {e}");
                            }
                        }
                        EgressResult::Segments { created, deleted } => {
                            if let Err(e) = self
                                .overseer
                                .on_segments(&self.connection.id, &created, &deleted)
                                .await
                            {
                                error!("Failed to notify overseer of deleted segments: {e}");
                            }
                        }
                        _ => {}
                    }
                }
            });
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

    /// Run pipeline ones, false=exit
    unsafe fn once(&mut self) -> Result<bool> {
        unsafe {
            if let Some(r) = self.handle_command()? {
                return Ok(r);
            }
            self.setup()?;

            // run transcoder pipeline
            let results = match &mut self.state {
                RunnerState::Normal => self.process_normal_mode()?,
                RunnerState::Shutdown => return Ok(false), // Shutdown requested
            };

            // egress results - process async operations without blocking if possible
            if !results.is_empty() {
                self.handle_egress_results(results);
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
                            StreamType::Video => IngressStreamType::Video,
                            StreamType::Audio => IngressStreamType::Audio,
                            StreamType::Subtitle => IngressStreamType::Subtitle,
                            StreamType::Unknown => IngressStreamType::Unknown,
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
        // setup scaler/encoders
        for out_stream in &cfg.variants {
            match out_stream {
                VariantStream::Video(v) => {
                    let encoder = v.create_encoder(true)?;
                    self.encoders.insert(out_stream.id(), encoder);
                    self.scalers.insert(out_stream.id(), Scaler::new());
                }
                VariantStream::Audio(a) => {
                    let enc = a.create_encoder(true)?;
                    let fmt = unsafe { transmute(a.sample_format_id()?) };
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
                        let stream = self.get_source_stream(var)?;
                        ret.streams.push(EncoderVariant {
                            variant: var,
                            stream,
                        });
                    }
                    if let Some(a) = v.audio {
                        let var = cfg.variants.iter().find(|x| x.id() == a)?;
                        let stream = self.get_source_stream(var)?;
                        ret.streams.push(EncoderVariant {
                            variant: var,
                            stream,
                        });
                    }
                    if let Some(s) = v.subtitle {
                        let var = cfg.variants.iter().find(|x| x.id() == s)?;
                        let stream = self.get_source_stream(var)?;
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
                    self.egress.push(Box::new(hls));
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
                        let rec = RecorderEgress::new(self.out_dir.clone(), a)?;
                        self.egress.push(Box::new(rec));
                    } else {
                        warn!(
                            "Could not find matching variant {}p, recording disabled!",
                            height
                        );
                    }
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
                #[cfg(feature = "egress-moq")]
                EgressType::Moq { .. } => {
                    let origin = self
                        .handle
                        .block_on(async { self.overseer.get_moq_origin().await })?;
                    let id = self.connection.id.to_string();
                    let moq = MoqEgress::new(self.handle.clone(), origin, &id, &variant_mapping)?;
                    self.egress.push(Box::new(moq));
                }
                #[allow(unreachable_patterns)]
                _ => bail!("Unhandled egress type: {:?}", e),
            }
        }
        Ok(())
    }

    fn get_source_stream(&self, v: &VariantStream) -> Option<EncoderOrSourceStream<'_>> {
        if let Some(e) = self.encoders.get(&v.id()) {
            Some(EncoderOrSourceStream::Encoder(e))
        } else {
            Some(EncoderOrSourceStream::SourceStream(unsafe {
                self.demuxer.get_stream(v.src_index()).ok()?
            }))
        }
    }
}

impl Drop for PipelineRunner {
    fn drop(&mut self) {
        unsafe {
            // First try to flush properly
            match self.flush() {
                Ok(r) => self.handle_egress_results(r),
                Err(e) => error!("Failed to flush pipeline during drop: {}", e),
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
