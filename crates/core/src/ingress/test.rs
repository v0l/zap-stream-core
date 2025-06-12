use crate::generator::FrameGenerator;
use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_free, av_packet_free, AVRational, AV_PROFILE_H264_MAIN,
};
use ffmpeg_rs_raw::{Encoder, Muxer};
use log::info;
use ringbuf::traits::{Observer, Split};
use ringbuf::{HeapCons, HeapRb};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use tiny_skia::Pixmap;
use tokio::runtime::Handle;
use uuid::Uuid;

pub async fn listen(out_dir: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    info!("Test pattern enabled");

    // add a delay, there is a race condition somewhere, the test pattern doesnt always
    // get added to active_streams
    tokio::time::sleep(Duration::from_secs(1)).await;

    let info = ConnectionInfo {
        id: Uuid::new_v4(),
        endpoint: "test-pattern",
        ip_addr: "test-pattern".to_string(),
        app_name: "".to_string(),
        key: "test".to_string(),
    };
    let src = TestPatternSrc::new()?;
    spawn_pipeline(
        Handle::current(),
        info,
        out_dir.clone(),
        overseer.clone(),
        Box::new(src),
    );
    Ok(())
}

struct TestPatternSrc {
    gen: FrameGenerator,
    video_encoder: Encoder,
    audio_encoder: Encoder,
    background: Pixmap,
    muxer: Muxer,
    reader: HeapCons<u8>,
}

unsafe impl Send for TestPatternSrc {}

const VIDEO_FPS: f32 = 30.0;
const VIDEO_WIDTH: u16 = 1280;
const VIDEO_HEIGHT: u16 = 720;
const SAMPLE_RATE: u32 = 44100;

impl TestPatternSrc {
    pub fn new() -> Result<Self> {
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
            gen: FrameGenerator::new(
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
        })
    }

    pub unsafe fn next_pkt(&mut self) -> Result<()> {
        self.gen.begin()?;
        self.gen.copy_frame_data(self.background.data())?;
        self.gen
            .write_text(&format!("frame={}", self.gen.frame_no()), 40.0, 5.0, 5.0)?;

        let mut frame = self.gen.next()?;
        if frame.is_null() {
            return Ok(());
        }

        // if sample_rate is set this frame is audio
        if (*frame).sample_rate > 0 {
            for mut pkt in self.audio_encoder.encode_frame(frame)? {
                self.muxer.write_packet(pkt)?;
                av_packet_free(&mut pkt);
            }
        } else {
            for mut pkt in self.video_encoder.encode_frame(frame)? {
                self.muxer.write_packet(pkt)?;
                av_packet_free(&mut pkt);
            }
        }

        av_frame_free(&mut frame);

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
