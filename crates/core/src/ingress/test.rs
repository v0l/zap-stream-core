use crate::generator::FrameGenerator;
use crate::ingress::{spawn_pipeline, ConnectionInfo, EndpointStats};
use crate::overseer::Overseer;
use crate::pipeline::runner::PipelineCommand;
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_free, av_packet_free, AVRational, AV_PROFILE_H264_MAIN,
};
use ffmpeg_rs_raw::{Encoder, Muxer};
use log::{info, warn};
use ringbuf::traits::{Observer, Split};
use ringbuf::{HeapCons, HeapRb};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use tiny_skia::Pixmap;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::time::Instant;
use uuid::Uuid;

pub async fn listen(out_dir: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    info!("Test pattern enabled");

    let info = ConnectionInfo {
        id: Uuid::new_v4(),
        endpoint: "test-pattern",
        ip_addr: "test-pattern".to_string(),
        app_name: "".to_string(),
        key: "test".to_string(),
    };
    let (tx, rx) = unbounded_channel();
    let src = TestPatternSrc::new(tx)?;
    spawn_pipeline(
        Handle::current(),
        info,
        out_dir,
        overseer,
        Box::new(src),
        None,
        Some(rx),
    );

    tokio::time::sleep(Duration::MAX).await;
    Ok(())
}

struct TestPatternSrc {
    frame_gen: FrameGenerator,
    video_encoder: Encoder,
    audio_encoder: Encoder,
    background: Pixmap,
    muxer: Muxer,
    reader: HeapCons<u8>,
    tx: UnboundedSender<PipelineCommand>,
    last_metrics: Instant,
    data_sent: u64,
}

unsafe impl Send for TestPatternSrc {}

const VIDEO_FPS: f32 = 30.0;
const VIDEO_WIDTH: u16 = 1280;
const VIDEO_HEIGHT: u16 = 720;
const SAMPLE_RATE: u32 = 44100;

impl TestPatternSrc {
    pub fn new(tx: UnboundedSender<PipelineCommand>) -> Result<Self> {
        let video_encoder = unsafe {
            Encoder::new_with_name("libx264")?
                .with_stream_index(0)
                .with_framerate(VIDEO_FPS)?
                .with_bitrate(1_000_000)
                .with_pix_fmt(AV_PIX_FMT_YUV420P)
                .with_width(VIDEO_WIDTH as _)
                .with_height(VIDEO_HEIGHT as _)
                .with_level(51)
                .with_profile(AV_PROFILE_H264_MAIN)
                .open(None)?
        };

        let audio_encoder = unsafe {
            Encoder::new_with_name("aac")?
                .with_stream_index(1)
                .with_default_channel_layout(1)
                .with_bitrate(128_000)
                .with_sample_format(AV_SAMPLE_FMT_FLTP)
                .with_sample_rate(SAMPLE_RATE as _)?
                .open(None)?
        };

        let svg_data = include_bytes!("../../test.svg");
        let tree = usvg::Tree::from_data(svg_data, &Default::default())?;

        let mut pixmap = Pixmap::new(VIDEO_WIDTH as _, VIDEO_HEIGHT as _).unwrap();
        let render_ts = tiny_skia::Transform::from_scale(
            pixmap.width() as f32 / tree.size().width(),
            pixmap.height() as f32 / tree.size().height(),
        );
        resvg::render(&tree, render_ts, &mut pixmap.as_mut());

        let buf = HeapRb::new(1024 * 1024);
        let (writer, reader) = buf.split();

        let muxer = unsafe {
            let mut m = Muxer::builder()
                .with_output_write(writer, Some("mpegts"))?
                .with_stream_encoder(&video_encoder)?
                .with_stream_encoder(&audio_encoder)?
                .build()?;
            m.open(None)?;
            m
        };

        let frame_size = unsafe { (*audio_encoder.codec_context()).frame_size as _ };
        Ok(Self {
            frame_gen: FrameGenerator::new(
                VIDEO_FPS,
                VIDEO_WIDTH,
                VIDEO_HEIGHT,
                AV_PIX_FMT_YUV420P,
                SAMPLE_RATE,
                frame_size,
                1,
                AVRational {
                    num: 1,
                    den: VIDEO_FPS as i32,
                },
                AVRational {
                    num: 1,
                    den: SAMPLE_RATE as i32,
                },
            )?,
            video_encoder,
            audio_encoder,
            muxer,
            background: pixmap,
            reader,
            tx,
            last_metrics: Instant::now(),
            data_sent: 0,
        })
    }

    pub unsafe fn next_pkt(&mut self) -> Result<()> {
        self.frame_gen.begin()?;
        self.frame_gen.copy_frame_data(self.background.data())?;
        self.frame_gen
            .write_text(&format!("frame={}", self.frame_gen.frame_no()), 40.0, 5.0, 5.0)?;

        let mut frame = self.frame_gen.next()?;
        if frame.is_null() {
            return Ok(());
        }

        // if sample_rate is set this frame is audio
        if (*frame).sample_rate > 0 {
            for mut pkt in self.audio_encoder.encode_frame(frame)? {
                self.data_sent += (*pkt).size as u64;
                self.muxer.write_packet(pkt)?;
                av_packet_free(&mut pkt);
            }
        } else {
            for mut pkt in self.video_encoder.encode_frame(frame)? {
                self.data_sent += (*pkt).size as u64;
                self.muxer.write_packet(pkt)?;
                av_packet_free(&mut pkt);
            }
        }

        av_frame_free(&mut frame);

        let metric_duration = Instant::now().duration_since(self.last_metrics);
        if metric_duration > Duration::from_secs(5) {
            if let Err(e) = self.tx.send(PipelineCommand::IngressMetrics(EndpointStats {
                name: "test".to_string(),
                bitrate: ((self.data_sent as f64 / metric_duration.as_secs_f64()) * 8.0) as usize,
            })) {
                warn!("Failed to send pipeline metrics: {}", e);
            }
            self.data_sent = 0;
            self.last_metrics = Instant::now();
        }
        Ok(())
    }
}

impl Read for TestPatternSrc {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        unsafe {
            while self.reader.occupied_len() < buf.len() {
                self.next_pkt().map_err(std::io::Error::other)?;
            }
        }
        self.reader.read(buf)
    }
}
