use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::mem::transmute;
use std::ops::Sub;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::egress::hls::HlsEgress;
use crate::egress::recorder::RecorderEgress;
use crate::egress::{Egress, EgressResult};
use crate::ingress::ConnectionInfo;
use crate::mux::SegmentType;
use crate::overseer::{IngressInfo, IngressStream, IngressStreamType, Overseer};
use crate::pipeline::{EgressType, PipelineConfig};
use crate::variant::{StreamMapping, VariantStream};
use crate::pipeline::placeholder::PlaceholderGenerator;
use anyhow::{bail, Result};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_WEBP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_free, av_get_sample_fmt, av_packet_free, av_q2d, av_rescale_q, AVMediaType,
};
use ffmpeg_rs_raw::{
    cstr, get_frame_from_hw, AudioFifo, Decoder, Demuxer, DemuxerInfo, Encoder, Resample, Scaler,
    StreamType,
};
use log::{error, info, warn};
use tokio::runtime::Handle;
use uuid::Uuid;

/// Runner state for handling normal vs idle modes
#[derive(Debug, Clone)]
pub enum RunnerState {
    /// Normal operation - processing live stream
    Normal,
    /// Idle mode - generating placeholder content after disconnection
    Idle {
        start_time: Instant,
        variant_index: usize,
        last_frame_time: Option<Instant>,
    },
}

/// Pipeline runner is the main entry process for stream transcoding
///
/// Each client connection spawns a new [PipelineRunner] and it should be run in its own thread
/// using [crate::ingress::spawn_pipeline]
pub struct PipelineRunner {
    /// Async runtime handle
    handle: Handle,

    /// Input stream connection info
    connection: ConnectionInfo,

    /// Configuration for this pipeline (variants, egress config etc.)
    config: Option<PipelineConfig>,

    /// Singleton demuxer for this input
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

    /// Info about the input stream
    info: Option<IngressInfo>,

    /// Overseer managing this pipeline
    overseer: Arc<dyn Overseer>,

    fps_counter_start: Instant,
    fps_last_frame_ctr: u64,

    /// Total number of frames produced
    frame_ctr: u64,
    out_dir: String,

    /// Thumbnail generation interval (0 = disabled)
    thumb_interval: u64,

    /// Current runner state (normal or idle)
    state: RunnerState,
}

impl PipelineRunner {
    pub fn new(
        handle: Handle,
        out_dir: String,
        overseer: Arc<dyn Overseer>,
        connection: ConnectionInfo,
        recv: Box<dyn Read + Send>,
    ) -> Result<Self> {
        Ok(Self {
            handle,
            out_dir,
            overseer,
            connection,
            config: Default::default(),
            demuxer: Demuxer::new_custom_io(recv, None)?,
            decoder: Decoder::new(),
            scalers: Default::default(),
            resampler: Default::default(),
            encoders: Default::default(),
            copy_stream: Default::default(),
            fps_counter_start: Instant::now(),
            egress: Vec::new(),
            frame_ctr: 0,
            fps_last_frame_ctr: 0,
            info: None,
            thumb_interval: 1800, // Disable thumbnails by default for performance
            state: RunnerState::Normal,
        })
    }

    /// Process a single idle frame - generates one source frame and processes it through all variants
    unsafe fn process_single_idle_frame(&mut self, config: &PipelineConfig) -> Result<()> {
        use std::time::{Duration, Instant};
        
        if config.variants.is_empty() {
            return Ok(());
        }

        // Extract timing info from current state
        let (mut last_frame_time, variant_index) = match &mut self.state {
            RunnerState::Idle { last_frame_time, variant_index, .. } => (last_frame_time, variant_index),
            _ => return Ok(()), // Only process in idle state
        };

        // Time-based frame rate calculation
        let now = Instant::now();
        if let Some(last_time) = *last_frame_time {
            // Calculate target frame interval (assume 30fps for now)
            let target_interval = Duration::from_millis(33); // ~30fps
            let elapsed = now.duration_since(last_time);
            
            if elapsed < target_interval {
                // Not time for next frame yet
                std::thread::sleep(target_interval - elapsed);
            }
        }
        *last_frame_time = Some(Instant::now());

        // Find the primary video variant to determine source frame properties
        let video_variant = config.variants.iter().find_map(|v| {
            if let VariantStream::Video(video) = v {
                Some(video)
            } else {
                None
            }
        });

        let mut egress_results = vec![];

        // Generate one source frame and process it through all relevant variants
        if let Some(video) = video_variant {
            // Generate a single source placeholder video frame
            let fps = if video.fps > 0.0 { video.fps } else { 30.0 };
            let time_base = (1, fps as i32);
            let mut source_frame = PlaceholderGenerator::generate_video_frame(video, time_base, self.frame_ctr)?;
            
            // Set the frame time_base
            (*source_frame).time_base.num = time_base.0;
            (*source_frame).time_base.den = time_base.1;
            
            // Increment frame counter for all video processing
            self.frame_ctr += 1;
            
            // Process this single frame through all video variants (like normal pipeline)
            for variant in &config.variants {
                if let VariantStream::Video(v) = variant {
                    // Scale/encode the source frame for this variant
                    if let Some(enc) = self.encoders.get_mut(&v.id()) {
                        // Use scaler if needed for different resolutions
                        let frame_to_encode = if v.width as i32 == (*source_frame).width && 
                                                v.height as i32 == (*source_frame).height {
                            // Same resolution, use source frame directly
                            source_frame
                        } else {
                            // Different resolution, need to scale
                            if let Some(scaler) = self.scalers.get_mut(&v.id()) {
                                scaler.process_frame(source_frame, v.width, v.height, AV_PIX_FMT_YUV420P)?
                            } else {
                                source_frame // Fallback to source frame
                            }
                        };

                        let packets = enc.encode_frame(frame_to_encode)?;
                        for mut pkt in packets {
                            for eg in self.egress.iter_mut() {
                                let er = eg.process_pkt(pkt, &v.id())?;
                                egress_results.push(er);
                            }
                            av_packet_free(&mut pkt);
                        }
                    }
                }
            }
            
            av_frame_free(&mut source_frame);
        }

        // Generate and process audio frames separately (audio doesn't share like video)
        for variant in &config.variants {
            if let VariantStream::Audio(a) = variant {
                let time_base = (1, a.sample_rate as i32);
                let mut frame = PlaceholderGenerator::generate_audio_frame(a, time_base, self.frame_ctr)?;
                
                // Set the frame time_base
                (*frame).time_base.num = time_base.0;
                (*frame).time_base.den = time_base.1;
                
                // Process through the encoding pipeline
                if let Some(enc) = self.encoders.get_mut(&a.id()) {
                    let packets = enc.encode_frame(frame)?;
                    for mut pkt in packets {
                        for eg in self.egress.iter_mut() {
                            let er = eg.process_pkt(pkt, &a.id())?;
                            egress_results.push(er);
                        }
                        av_packet_free(&mut pkt);
                    }
                }
                
                av_frame_free(&mut frame);
            }
        }
        
        // Handle egress results (same as normal processing)
        if !egress_results.is_empty() {
            self.handle.block_on(async {
                for er in egress_results {
                    if let EgressResult::Segments { created, deleted } = er {
                        if let Err(e) = self
                            .overseer
                            .on_segments(&config.id, &created, &deleted)
                            .await
                        {
                            bail!("Failed to process segment {}", e.to_string());
                        }
                    }
                }
                Ok(())
            })?;
        }
        
        Ok(())
    }

    /// EOF, cleanup
    pub unsafe fn flush(&mut self) -> Result<()> {
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

        if let Some(config) = &self.config {
            self.handle.block_on(async {
                if let Err(e) = self.overseer.on_end(&config.id).await {
                    error!("Failed to end stream: {e}");
                }
            });
        }
        Ok(())
    }

    /// Main processor, should be called in a loop
    /// Returns false when stream data ended (EOF)
    pub unsafe fn run(&mut self) -> Result<bool> {
        self.setup()?;

        let config = if let Some(config) = &self.config {
            config
        } else {
            bail!("Pipeline not configured, cannot run")
        };

        // run transcoder pipeline
        let (mut pkt, stream_info) = self.demuxer.get_packet()?;
        
        // Handle state transitions based on packet availability
        match (&self.state, pkt.is_null()) {
            (RunnerState::Normal, true) => {
                // First time entering idle mode
                info!("Stream input disconnected, entering idle mode with placeholder content");
                self.state = RunnerState::Idle {
                    start_time: Instant::now(),
                    variant_index: 0,
                    last_frame_time: None,
                };
            }
            (RunnerState::Idle { start_time, .. }, true) => {
                // Check if we've been idle for more than 1 minute
                if start_time.elapsed() > Duration::from_secs(60) {
                    info!("Idle timeout reached (60 seconds), ending stream");
                    return Ok(false);
                }
            }
            (RunnerState::Idle { .. }, false) => {
                // Stream reconnected
                info!("Stream reconnected, leaving idle mode");
                self.state = RunnerState::Normal;
            }
            (RunnerState::Normal, false) => {
                // Normal operation continues
            }
        }
        
        // Process based on current state
        match &self.state {
            RunnerState::Idle { .. } => {
                // Process a single idle frame (rotating through variants)
                self.process_single_idle_frame(config)?;
                
                // Free the null packet if needed
                if !pkt.is_null() {
                    av_packet_free(&mut pkt);
                }
                
                return Ok(true); // Continue processing
            }
            RunnerState::Normal => {
                // Normal packet processing
                if pkt.is_null() {
                    // This shouldn't happen in Normal state but handle gracefully
                    return Ok(true);
                }
                
                // TODO: For copy streams, skip decoder
                let frames = match self.decoder.decode_pkt(pkt) {
                    Ok(f) => f,
                    Err(e) => {
                        warn!("Error decoding frames, {e}");
                        return Ok(true);
                    }
                };

                let mut egress_results = vec![];
                for (frame, stream) in frames {
            // Copy frame from GPU if using hwaccel decoding
            let mut frame = get_frame_from_hw(frame)?;
            (*frame).time_base = (*stream).time_base;

            let p = (*stream).codecpar;
            if (*p).codec_type == AVMediaType::AVMEDIA_TYPE_VIDEO {
                // Conditionally generate thumbnails based on interval (0 = disabled)
                if self.thumb_interval > 0 && (self.frame_ctr % self.thumb_interval) == 0 {
                    let thumb_start = Instant::now();
                    let dst_pic = PathBuf::from(&self.out_dir)
                        .join(config.id.to_string())
                        .join("thumb.webp");
                    {
                        let mut sw = Scaler::new();
                        let mut scaled_frame = sw.process_frame(
                            frame,
                            (*frame).width as _,
                            (*frame).height as _,
                            AV_PIX_FMT_YUV420P,
                        )?;

                        let mut encoder = Encoder::new(AV_CODEC_ID_WEBP)?
                            .with_height((*scaled_frame).height)
                            .with_width((*scaled_frame).width)
                            .with_pix_fmt(transmute((*scaled_frame).format))
                            .open(None)?;

                        encoder.save_picture(scaled_frame, dst_pic.to_str().unwrap())?;
                        av_frame_free(&mut scaled_frame);
                    }

                    let thumb_duration = thumb_start.elapsed();
                    info!(
                        "Saved thumb ({:.2}ms) to: {}",
                        thumb_duration.as_millis() as f32 / 1000.0,
                        dst_pic.display(),
                    );
                }

                self.frame_ctr += 1;
            }

            // Get the variants which want this pkt
            let pkt_vars = config
                .variants
                .iter()
                .filter(|v| v.src_index() == (*stream).index as usize);
            for var in pkt_vars {
                let enc = if let Some(enc) = self.encoders.get_mut(&var.id()) {
                    enc
                } else {
                    //warn!("Frame had nowhere to go in {} :/", var.id());
                    continue;
                };

                // scaling / resampling
                let mut new_frame = false;
                let mut frame = match var {
                    VariantStream::Video(v) => {
                        if let Some(s) = self.scalers.get_mut(&v.id()) {
                            new_frame = true;
                            s.process_frame(frame, v.width, v.height, transmute(v.pixel_format))?
                        } else {
                            frame
                        }
                    }
                    VariantStream::Audio(a) => {
                        if let Some((r, f)) = self.resampler.get_mut(&a.id()) {
                            let frame_size = (*enc.codec_context()).frame_size;
                            new_frame = true;
                            let mut resampled_frame = r.process_frame(frame)?;
                            if let Some(ret) =
                                f.buffer_frame(resampled_frame, frame_size as usize)?
                            {
                                // Set correct timebase for audio (1/sample_rate)
                                (*ret).time_base.num = 1;
                                (*ret).time_base.den = a.sample_rate as i32;
                                av_frame_free(&mut resampled_frame);
                                ret
                            } else {
                                av_frame_free(&mut resampled_frame);
                                continue;
                            }
                        } else {
                            frame
                        }
                    }
                    _ => frame,
                };

                // before encoding frame, rescale timestamps
                if !frame.is_null() {
                    let enc_ctx = enc.codec_context();
                    (*frame).pict_type = AV_PICTURE_TYPE_NONE;
                    (*frame).pts =
                        av_rescale_q((*frame).pts, (*frame).time_base, (*enc_ctx).time_base);
                    (*frame).pkt_dts =
                        av_rescale_q((*frame).pkt_dts, (*frame).time_base, (*enc_ctx).time_base);
                    (*frame).duration =
                        av_rescale_q((*frame).duration, (*frame).time_base, (*enc_ctx).time_base);
                    (*frame).time_base = (*enc_ctx).time_base;
                }

                let packets = enc.encode_frame(frame)?;
                // pass new packets to egress
                for mut pkt in packets {
                    for eg in self.egress.iter_mut() {
                        let er = eg.process_pkt(pkt, &var.id())?;
                        egress_results.push(er);
                    }
                    av_packet_free(&mut pkt);
                }

                if new_frame {
                    av_frame_free(&mut frame);
                }
            }

            av_frame_free(&mut frame);
        }

        av_packet_free(&mut pkt);

        // egress results - process async operations without blocking if possible
        if !egress_results.is_empty() {
            self.handle.block_on(async {
                for er in egress_results {
                    if let EgressResult::Segments { created, deleted } = er {
                        if let Err(e) = self
                            .overseer
                            .on_segments(&config.id, &created, &deleted)
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
            info!("Average fps: {:.2}", n_frames as f32 / elapsed);
            self.fps_counter_start = Instant::now();
            self.fps_last_frame_ctr = self.frame_ctr;
        }
            } // Close the RunnerState::Normal match arm
        }
        Ok(true)
    }

    unsafe fn setup(&mut self) -> Result<()> {
        if self.info.is_some() {
            return Ok(());
        }

        let info = self.demuxer.probe_input()?;

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
                    language: s.language.clone(),
                })
                .collect(),
        };

        let cfg = self
            .handle
            .block_on(async { self.overseer.start_stream(&self.connection, &i_info).await })?;
        self.config = Some(cfg);
        self.info = Some(i_info);

        self.setup_pipeline(&info)?;
        Ok(())
    }

    unsafe fn setup_pipeline(&mut self, demux_info: &DemuxerInfo) -> Result<()> {
        let cfg = if let Some(ref cfg) = self.config {
            cfg
        } else {
            bail!("Cannot setup pipeline without config");
        };

        // src stream indexes
        let inputs: HashSet<usize> = cfg.variants.iter().map(|e| e.src_index()).collect();

        // enable hardware decoding
        self.decoder.enable_hw_decoder_any();

        // setup decoders
        for input_idx in inputs {
            let stream = demux_info
                .streams
                .iter()
                .find(|f| f.index == input_idx)
                .unwrap();
            self.decoder.setup_decoder(stream, None)?;
        }

        // setup scaler/encoders
        for out_stream in &cfg.variants {
            match out_stream {
                VariantStream::Video(v) => {
                    self.encoders.insert(out_stream.id(), v.try_into()?);
                    self.scalers.insert(out_stream.id(), Scaler::new());
                }
                VariantStream::Audio(a) => {
                    let enc = a.try_into()?;
                    let fmt = av_get_sample_fmt(cstr!(a.sample_fmt.as_str()));
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
                    let hls =
                        HlsEgress::new(&cfg.id, &self.out_dir, 2.0, encoders, SegmentType::MPEGTS)?;
                    self.egress.push(Box::new(hls));
                }
                EgressType::Recorder(_) => {
                    let rec = RecorderEgress::new(&cfg.id, &self.out_dir, encoders)?;
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

            info!("PipelineRunner cleaned up resources for stream: {}", self.connection.key);
        }
    }
}
