use crate::egress::{Egress, EgressResult};
use crate::overseer::Overseer;
use crate::variant::VariantStream;
use anyhow::{Result, anyhow, bail};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID::AV_CODEC_ID_WEBP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AV_NOPTS_VALUE, av_rescale_q};
use ffmpeg_rs_raw::{AudioFifo, AvFrameRef, AvPacketRef, Encoder, Resample, Scaler};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::runtime::Handle;
use tracing::{error, info, trace, warn};
use uuid::Uuid;

#[derive(Clone)]
pub enum WorkerThreadCommand {
    /// Scale/resample and Encode a frame
    EncodeFrame { frame: AvFrameRef },
    /// Save a thumbnail from a given frame
    SaveThumbnail { frame: AvFrameRef, dst: PathBuf },
    /// Send packets to the egress'
    MuxPacket { packet: AvPacketRef },
    /// Flush worker thread for shutdown
    Flush,
}

/// A pipeline worker thread handles all the frames/packets for a specific variant
pub struct PipelineWorkerThread {
    pipeline_id: Uuid,
    handle: Handle,

    variant: VariantStream,
    scaler: Option<Scaler>,
    encoder: Option<Encoder>,
    resampler: Option<(Resample, AudioFifo)>,

    egress: Arc<Mutex<Vec<Box<dyn Egress>>>>,
    overseer: Arc<dyn Overseer>,

    work_queue_tx: Option<Sender<WorkerThreadCommand>>,
    work_queue_rx: Receiver<WorkerThreadCommand>,

    did_flush: bool,
}

impl PipelineWorkerThread {
    pub fn sender(&mut self) -> Option<Sender<WorkerThreadCommand>> {
        self.work_queue_tx.take()
    }

    fn generate_thumbnail(&self, frame: AvFrameRef, dst_pic: PathBuf) -> Result<()> {
        let thumb_start = Instant::now();

        let encoder = unsafe {
            Encoder::new(AV_CODEC_ID_WEBP)?
                .with_height(frame.height)
                .with_width(frame.width)
                .with_pix_fmt(AV_PIX_FMT_YUV420P)
                .open(None)?
        };

        // use scaler to convert pixel format if not YUV420P
        if frame.format as i32 != AV_PIX_FMT_YUV420P as i32 {
            let mut sw = Scaler::new();
            let new_frame = sw.process_frame(
                &frame,
                frame.width as _,
                frame.height as _,
                AV_PIX_FMT_YUV420P,
            )?;
            encoder.save_picture(&new_frame, dst_pic.to_str().unwrap())?;
        } else {
            encoder.save_picture(&frame, dst_pic.to_str().unwrap())?;
        };

        let thumb_duration = thumb_start.elapsed();
        info!(
            "Saved thumb ({}ms) to: {}",
            thumb_duration.as_millis(),
            dst_pic.display(),
        );
        let block_on_start = Instant::now();
        if let Err(e) = self.handle.block_on(self.overseer.on_thumbnail(
            &self.pipeline_id,
            frame.width as _,
            frame.height as _,
            &dst_pic,
        )) {
            warn!("Failed to handle on_thumbnail: {}", e);
        }
        crate::metrics::record_thumbnail_generation_time(thumb_duration);
        crate::metrics::record_block_on_thumbnail(block_on_start.elapsed());
        Ok(())
    }

    fn egress_packets(&self, packets: Vec<AvPacketRef>) -> Result<()> {
        let mut egress = self.egress.lock().expect("egress lock poisoned");
        let mut results = vec![];
        let var_id = self.variant.id();
        for pkt in packets {
            for eg in egress.iter_mut() {
                trace!(
                    "EGRESS PKT: var={}, idx={}, pts={}, dts={}, dur={}",
                    &var_id, pkt.stream_index, pkt.pts, pkt.dts, pkt.duration
                );
                let er = eg.process_pkt(pkt.clone(), &var_id)?;
                results.push(er);
            }
        }
        drop(egress);
        self.handle_egress_results(results);
        Ok(())
    }

    fn handle_egress_results(&self, results: Vec<EgressResult>) {
        if results.is_empty() {
            return;
        }
        // Process reset results and notify overseer of deleted segments
        let block_on_start = Instant::now();
        self.handle.block_on(async {
            for result in results {
                match result {
                    EgressResult::Flush => {
                        if let Err(e) = self.overseer.on_end(&self.pipeline_id).await {
                            error!("Failed to end stream: {e}");
                        }
                    }
                    EgressResult::Segments { created, deleted } => {
                        if let Err(e) = self
                            .overseer
                            .on_segments(&self.pipeline_id, &created, &deleted)
                            .await
                        {
                            error!("Failed to notify overseer of deleted segments: {e}");
                        }
                    }
                    _ => {}
                }
            }
        });
        crate::metrics::record_block_on_egress_results(block_on_start.elapsed());
    }

    fn scale_encode_frame(&mut self, frame: AvFrameRef) -> Result<()> {
        if let Some(s) = &mut self.scaler {
            let vv = match &self.variant {
                VariantStream::Video(v) => v,
                _ => bail!("Tried to scale/encode a frame for the wrong stream type"),
            };
            trace!(
                "FRAME->SCALER: var={}, pts={}, pkt_dts={}, duration={}, tb={}/{}",
                vv.id,
                frame.pts,
                frame.pkt_dts,
                frame.duration,
                frame.time_base.num,
                frame.time_base.den
            );

            let Ok(pix_fmt) = vv.pixel_format_id() else {
                bail!("Could not scale frame without pixel_format for {}", vv.id);
            };
            let new_frame = s.process_frame(&frame, vv.width, vv.height, unsafe {
                std::mem::transmute(pix_fmt)
            })?;

            self.encode_mux_frame(Some(new_frame))?;
        } else {
            self.encode_mux_frame(Some(frame))?;
        }
        Ok(())
    }

    fn encode_mux_frame(&mut self, mut frame: Option<AvFrameRef>) -> Result<()> {
        let Some(e) = &mut self.encoder else {
            bail!("Tried to encode a frame without and encoder setup!");
        };

        // before encoding frame, rescale timestamps
        if let Some(frame) = frame.as_mut() {
            let enc_ctx = e.codec_context();
            frame.pict_type = AV_PICTURE_TYPE_NONE;
            frame.pts = unsafe { av_rescale_q(frame.pts, frame.time_base, (*enc_ctx).time_base) };
            frame.pkt_dts = AV_NOPTS_VALUE;
            frame.duration =
                unsafe { av_rescale_q(frame.duration, frame.time_base, (*enc_ctx).time_base) };
            frame.time_base = unsafe { (*enc_ctx).time_base };

            trace!(
                "FRAME->ENCODER: var={}, pts={}, duration={}, tb={}/{}",
                self.variant.id(),
                frame.pts,
                frame.duration,
                frame.time_base.num,
                frame.time_base.den
            );
        }

        let packets = match e.encode_frame(frame.as_ref()) {
            Ok(pkt) => pkt,
            Err(e) => {
                if let Some(frame) = frame {
                    error!(
                        "Failed to encode frame: var={}, pts={}, duration={} {}",
                        self.variant.id(),
                        frame.pts,
                        frame.duration,
                        e
                    );
                } else {
                    error!(
                        "Failed to encode frame: var={}, flush={{true}} {}",
                        self.variant.id(),
                        e
                    );
                }
                return Err(e);
            }
        };
        trace!("ENCODER->PKTS: var={}", self.variant.id());
        self.egress_packets(packets)?;
        Ok(())
    }

    fn resample_encode_frame(&mut self, frame: Option<AvFrameRef>) -> Result<()> {
        let flush = frame.is_none();
        let frames = if let Some((r, f)) = &mut self.resampler {
            let (frame_size, sample_rate) = {
                let Some(e) = &self.encoder else {
                    bail!("Tried to encode a frame without and encoder setup!");
                };
                unsafe {
                    (
                        (*e.codec_context()).frame_size,
                        (*e.codec_context()).sample_rate,
                    )
                }
            };
            if let Some(frame) = frame {
                let resampled_frame = r.process_frame(&frame)?;
                f.buffer_frame(&resampled_frame)?;
            }
            let mut ret = vec![];
            // drain FIFO
            while let Some(mut frame) = f.get_frame(frame_size as usize)? {
                // Set correct timebase for audio (1/sample_rate)
                frame.time_base.num = 1;
                frame.time_base.den = sample_rate as i32;
                ret.push(frame);
            }
            ret
        } else {
            if let Some(frame) = frame {
                vec![frame]
            } else {
                vec![]
            }
        };
        for frame in frames {
            self.encode_mux_frame(Some(frame))?;
        }
        if flush {
            self.encode_mux_frame(None)?;
        }

        Ok(())
    }

    fn process_msg(&mut self, msg: WorkerThreadCommand) -> Result<()> {
        match msg {
            WorkerThreadCommand::EncodeFrame { frame } => match &self.variant {
                VariantStream::Video(_) => {
                    self.scale_encode_frame(frame)?;
                }
                VariantStream::Audio(_) => {
                    self.resample_encode_frame(Some(frame))?;
                }
                _ => warn!("Got encode frame for copy variant!"),
            },
            WorkerThreadCommand::SaveThumbnail { frame, dst } => {
                self.generate_thumbnail(frame, dst)?;
            }
            WorkerThreadCommand::MuxPacket { packet } => {
                self.egress_packets(vec![packet])?;
            }
            WorkerThreadCommand::Flush => {
                self.did_flush = true;
                match self.variant {
                    VariantStream::Video(_) => {
                        self.encode_mux_frame(None)?;
                    }
                    VariantStream::Audio(_) => {
                        self.resample_encode_frame(None)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Start the worker thread
    pub fn run(mut self) -> Result<()> {
        info!("Worker thread starting for variant: {}", self.variant.id());

        if let Err(e) = std::thread::Builder::new()
            .name(format!(
                "pipeline:{}:worker:{}",
                self.pipeline_id,
                self.variant.id()
            ))
            .spawn(move || {
                loop {
                    match self.work_queue_rx.recv() {
                        Ok(msg) => {
                            if let Err(e) = self.process_msg(msg) {
                                error!("Failed to process message: {}", e);
                            }
                        }
                        Err(e) => {
                            if !self.did_flush {
                                error!("Channel closed before flush command! {e}");
                            }
                            break;
                        }
                    }
                }
                info!("Worker thread terminated");
            })
        {
            bail!("Failed to start worker: {e}");
        }
        Ok(())
    }
}

pub struct PipelineWorkerThreadBuilder {
    pipeline_id: Option<Uuid>,
    handle: Option<Handle>,
    egress: Option<Arc<Mutex<Vec<Box<dyn Egress>>>>>,
    overseer: Option<Arc<dyn Overseer>>,

    variant: VariantStream,
    scaler: Option<Scaler>,
    encoder: Option<Encoder>,
    resampler: Option<(Resample, AudioFifo)>,
    work_queue_tx: Sender<WorkerThreadCommand>,
    work_queue_rx: Receiver<WorkerThreadCommand>,
}

impl PipelineWorkerThreadBuilder {
    pub fn encoder(&self) -> Option<&Encoder> {
        self.encoder.as_ref()
    }

    pub fn with_pipeline_id(mut self, pipeline_id: Uuid) -> Self {
        self.pipeline_id = Some(pipeline_id);
        self
    }

    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.handle = Some(handle);
        self
    }

    pub fn with_egress(mut self, egress: Arc<Mutex<Vec<Box<dyn Egress>>>>) -> Self {
        self.egress = Some(egress);
        self
    }

    pub fn with_overseer(mut self, overseer: Arc<dyn Overseer>) -> Self {
        self.overseer = Some(overseer);
        self
    }

    pub fn build(self) -> Result<PipelineWorkerThread> {
        Ok(PipelineWorkerThread {
            pipeline_id: self.pipeline_id.ok_or(anyhow!("Pipeline ID not set"))?,
            handle: self.handle.ok_or(anyhow!("Handle not set"))?,
            variant: self.variant,
            scaler: self.scaler,
            encoder: self.encoder,
            resampler: self.resampler,
            egress: self.egress.ok_or(anyhow!("Egress not set"))?,
            overseer: self.overseer.ok_or(anyhow!("Overseer not set"))?,
            work_queue_tx: Some(self.work_queue_tx),
            work_queue_rx: self.work_queue_rx,
            did_flush: false,
        })
    }
}

impl TryFrom<&VariantStream> for PipelineWorkerThreadBuilder {
    type Error = anyhow::Error;
    fn try_from(value: &VariantStream) -> Result<Self, Self::Error> {
        match value {
            VariantStream::Video(v) => {
                let enc = v.create_encoder(true)?;
                let (tx, rx) = channel();
                Ok(Self {
                    pipeline_id: None,
                    handle: None,
                    egress: None,
                    overseer: None,
                    variant: value.clone(),
                    scaler: if let Some(_) = v.scale_mode {
                        Some(Scaler::default())
                    } else {
                        None
                    },
                    encoder: Some(enc),
                    resampler: None,
                    work_queue_tx: tx,
                    work_queue_rx: rx,
                })
            }
            VariantStream::Audio(a) => {
                let enc = a.create_encoder(true)?;
                let fmt = unsafe { std::mem::transmute(a.sample_format_id()?) };
                let rs = Resample::new(fmt, a.sample_rate as _, a.channels as _);
                let f = AudioFifo::new(fmt, a.channels as _)?;
                let (tx, rx) = channel();
                Ok(Self {
                    pipeline_id: None,
                    handle: None,
                    egress: None,
                    overseer: None,
                    variant: value.clone(),
                    scaler: None,
                    encoder: Some(enc),
                    resampler: Some((rs, f)),
                    work_queue_tx: tx,
                    work_queue_rx: rx,
                })
            }
            VariantStream::Subtitle { .. } => todo!(),
            VariantStream::CopyVideo(_) | VariantStream::CopyAudio(_) => {
                let (tx, rx) = channel();
                Ok(Self {
                    pipeline_id: None,
                    handle: None,
                    egress: None,
                    overseer: None,
                    variant: value.clone(),
                    scaler: None,
                    encoder: None,
                    resampler: None,
                    work_queue_tx: tx,
                    work_queue_rx: rx,
                })
            }
        }
    }
}
