use crate::ingress::{spawn_pipeline, ConnectionInfo};
use crate::overseer::Overseer;
use anyhow::Result;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVColorSpace::AVCOL_SPC_RGB;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVPixelFormat::{AV_PIX_FMT_RGBA, AV_PIX_FMT_YUV420P};
use ffmpeg_rs_raw::ffmpeg_sys_the_third::{
    av_frame_alloc, av_frame_free, av_frame_get_buffer, av_packet_free, AV_PROFILE_H264_MAIN,
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
    encoder: Encoder,
    scaler: Scaler,
    muxer: Muxer,
    background: Pixmap,
    font: [Font; 1],
    frame_no: u64,
    start: Instant,
    reader: HeapCons<u8>,
}

unsafe impl Send for TestPatternSrc {}

impl TestPatternSrc {
    pub fn new() -> Result<Self> {
        let scaler = Scaler::new();
        let encoder = unsafe {
            Encoder::new_with_name("libx264")?
                .with_stream_index(0)
                .with_framerate(30.0)?
                .with_bitrate(1_000_000)
                .with_pix_fmt(AV_PIX_FMT_YUV420P)
                .with_width(1280)
                .with_height(720)
                .with_level(51)
                .with_profile(AV_PROFILE_H264_MAIN)
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
                .with_stream_encoder(&encoder)?
                .build()?;
            m.open(None)?;
            m
        };

        Ok(Self {
            encoder,
            scaler,
            muxer,
            background: pixmap,
            font: [font],
            frame_no: 0,
            start: Instant::now(),
            reader,
        })
    }

    pub unsafe fn next_pkt(&mut self) -> Result<()> {
        let stream_time = Duration::from_secs_f64(self.frame_no as f64 / 30.0);
        let real_time = Instant::now().duration_since(self.start);
        let wait_time = if stream_time > real_time {
            stream_time - real_time
        } else {
            Duration::new(0, 0)
        };
        if !wait_time.is_zero() {
            std::thread::sleep(wait_time);
        }

        self.frame_no += 1;

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

        // scale/encode
        let mut frame = self
            .scaler
            .process_frame(src_frame, 1280, 720, AV_PIX_FMT_YUV420P)?;
        for mut pkt in self.encoder.encode_frame(frame)? {
            self.muxer.write_packet(pkt)?;
            av_packet_free(&mut pkt);
        }
        av_frame_free(&mut frame);
        av_frame_free(&mut src_frame);
        Ok(())
    }
}

impl Read for TestPatternSrc {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        unsafe {
            while self.reader.occupied_len() < buf.len() {
                self.next_pkt().map_err(|e| std::io::Error::other(e))?;
            }
        }
        self.reader.read(buf)
    }
}
