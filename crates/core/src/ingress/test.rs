use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_RGB;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::{AV_PIX_FMT_RGBA, AV_PIX_FMT_YUV420P};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_alloc, av_frame_free, av_frame_get_buffer, av_packet_free, AVRational,
    AV_PROFILE_H264_MAIN,
};
use ffmpeg_rs_raw::{Encoder, Muxer, Scaler};
use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
use fontdue::Font;
use log::info;
use ringbuf::traits::{Observer, Split};
use ringbuf::{HeapCons, HeapRb};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;
use tokio::runtime::Handle;

pub async fn listen(out_dir: String, overseer: Arc<dyn Overseer>) -> Result<()> {
    info!("Test pattern enabled");

    let info = ConnectionInfo {
        endpoint: "test-pattern".to_string(),
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
    video_encoder: Encoder,
    audio_encoder: Encoder,
    scaler: Scaler,
    muxer: Muxer,
    background: Pixmap,
    font: [Font; 1],
    frame_no: u64,
    audio_sample_no: u64,
    start: Instant,
    reader: HeapCons<u8>,
}

unsafe impl Send for TestPatternSrc {}

const VIDEO_FPS: f32 = 30.0;

impl TestPatternSrc {
    pub fn new() -> Result<Self> {
        let scaler = Scaler::new();
        let video_encoder = unsafe {
            Encoder::new_with_name("libx264")?
                .with_stream_index(0)
                .with_framerate(VIDEO_FPS)?
                .with_bitrate(1_000_000)
                .with_pix_fmt(AV_PIX_FMT_YUV420P)
                .with_width(1280)
                .with_height(720)
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
                .with_sample_rate(44100)?
                .open(None)?
        };

        let svg_data = include_bytes!("../../test.svg");
        let tree = usvg::Tree::from_data(svg_data, &Default::default())?;
        let mut pixmap = Pixmap::new(1280, 720).unwrap();
        let render_ts = tiny_skia::Transform::from_scale(
            pixmap.width() as f32 / tree.size().width(),
            pixmap.height() as f32 / tree.size().height(),
        );
        resvg::render(&tree, render_ts, &mut pixmap.as_mut());

        let font = include_bytes!("../../SourceCodePro-Regular.ttf") as &[u8];
        let font = Font::from_bytes(font, Default::default()).unwrap();

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

        Ok(Self {
            video_encoder,
            audio_encoder,
            scaler,
            muxer,
            background: pixmap,
            font: [font],
            frame_no: 0,
            audio_sample_no: 0,
            start: Instant::now(),
            reader,
        })
    }

    pub unsafe fn next_pkt(&mut self) -> Result<()> {
        let stream_time = Duration::from_secs_f64(self.frame_no as f64 / VIDEO_FPS as f64);
        let real_time = Instant::now().duration_since(self.start);
        let wait_time = if stream_time > real_time {
            stream_time - real_time
        } else {
            Duration::new(0, 0)
        };
        if !wait_time.is_zero() && wait_time.as_secs_f32() > 1f32 / VIDEO_FPS {
            std::thread::sleep(wait_time);
        }

        let mut src_frame = unsafe {
            let src_frame = av_frame_alloc();

            (*src_frame).width = 1280;
            (*src_frame).height = 720;
            (*src_frame).pict_type = AV_PICTURE_TYPE_NONE;
            (*src_frame).key_frame = 1;
            (*src_frame).colorspace = AVCOL_SPC_RGB;
            (*src_frame).format = AV_PIX_FMT_RGBA as _;
            (*src_frame).pts = self.frame_no as i64;
            (*src_frame).duration = 1;
            av_frame_get_buffer(src_frame, 0);

            self.background
                .data()
                .as_ptr()
                .copy_to((*src_frame).data[0] as *mut _, 1280 * 720 * 4);
            src_frame
        };
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.clear();
        layout.append(
            &self.font,
            &TextStyle::new(&format!("frame={}", self.frame_no), 40.0, 0),
        );
        for g in layout.glyphs() {
            let (metrics, bitmap) = self.font[0].rasterize_config_subpixel(g.key);
            for y in 0..metrics.height {
                for x in 0..metrics.width {
                    let dst_x = x + g.x as usize;
                    let dst_y = y + g.y as usize;
                    let offset_src = (x + y * metrics.width) * 3;
                    unsafe {
                        let offset_dst = 4 * dst_x + dst_y * (*src_frame).linesize[0] as usize;
                        let pixel_dst = (*src_frame).data[0].add(offset_dst);
                        *pixel_dst.offset(0) = bitmap[offset_src];
                        *pixel_dst.offset(1) = bitmap[offset_src + 1];
                        *pixel_dst.offset(2) = bitmap[offset_src + 2];
                    }
                }
            }
        }

        // scale/encode video
        let mut frame = self
            .scaler
            .process_frame(src_frame, 1280, 720, AV_PIX_FMT_YUV420P)?;
        for mut pkt in self.video_encoder.encode_frame(frame)? {
            self.muxer.write_packet(pkt)?;
            av_packet_free(&mut pkt);
        }
        av_frame_free(&mut frame);
        av_frame_free(&mut src_frame);

        // Generate and encode audio (sine wave)
        self.generate_audio_frame()?;

        self.frame_no += 1;

        Ok(())
    }

    /// Generate audio to stay synchronized with video frames
    unsafe fn generate_audio_frame(&mut self) -> Result<()> {
        const SAMPLE_RATE: f32 = 44100.0;
        const FREQUENCY: f32 = 440.0; // A4 note
        const SAMPLES_PER_FRAME: usize = 1024; // Fixed AAC frame size

        // Calculate how many audio samples we should have by now
        // At 30fps, each video frame = 1/30 sec = 1470 audio samples at 44.1kHz
        let audio_samples_per_video_frame = (SAMPLE_RATE / VIDEO_FPS) as u64; // ~1470 samples
        let target_audio_samples = self.frame_no * audio_samples_per_video_frame;

        // Generate audio frames to catch up to the target
        while self.audio_sample_no < target_audio_samples {
            let mut audio_frame = av_frame_alloc();
            (*audio_frame).format = AV_SAMPLE_FMT_FLTP as _;
            (*audio_frame).nb_samples = SAMPLES_PER_FRAME as _;
            (*audio_frame).ch_layout.nb_channels = 1;
            (*audio_frame).sample_rate = SAMPLE_RATE as _;
            (*audio_frame).pts = self.audio_sample_no as i64;
            (*audio_frame).duration = 1;
            (*audio_frame).time_base = AVRational {
                num: 1,
                den: SAMPLE_RATE as _,
            };

            av_frame_get_buffer(audio_frame, 0);

            // Generate sine wave samples
            let data = (*audio_frame).data[0] as *mut f32;
            for i in 0..SAMPLES_PER_FRAME {
                let sample_time = (self.audio_sample_no + i as u64) as f32 / SAMPLE_RATE;
                let sample_value =
                    (2.0 * std::f32::consts::PI * FREQUENCY * sample_time).sin() * 0.5;
                *data.add(i) = sample_value;
            }

            // Encode audio frame
            for mut pkt in self.audio_encoder.encode_frame(audio_frame)? {
                self.muxer.write_packet(pkt)?;
                av_packet_free(&mut pkt);
            }

            self.audio_sample_no += SAMPLES_PER_FRAME as u64;
            av_frame_free(&mut audio_frame);
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
