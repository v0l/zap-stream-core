use std::ops::Add;
use std::slice;
use std::time::{Duration, Instant};

use crate::encode::video::VideoEncoder;
use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;
use crate::pipeline::{AVFrameSource, PipelinePayload, PipelineProcessor};
use crate::scale::Scaler;
use crate::variant::mapping::VariantMapping;
use crate::variant::video::VideoVariant;
use ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_RGB;
use ffmpeg_sys_next::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_sys_next::AVPixelFormat::{AV_PIX_FMT_RGBA, AV_PIX_FMT_YUV420P};
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_get_buffer, AV_PROFILE_H264_MAIN,
};
use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
use libc::memcpy;
use log::{error, info, warn};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use uuid::Uuid;

const WIDTH: libc::c_int = 1920;
const HEIGHT: libc::c_int = 1080;
const FPS: libc::c_int = 25;

pub async fn listen(builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    info!("Test pattern enabled");

    let (tx, rx) = unbounded_channel();
    let info = ConnectionInfo {
        ip_addr: "".to_owned(),
        endpoint: "test-pattern".to_owned(),
    };

    if let Ok(mut pl) = builder.build_for(info, rx).await {
        let pipeline = std::thread::spawn(move || loop {
            if let Err(e) = pl.run() {
                error!("Pipeline error: {}\n{}", e, e.backtrace());
                break;
            }
        });
        let encoder = std::thread::spawn(move || {
            run_encoder(tx);
        });
        if encoder.join().is_err() {
            error!("Encoder thread error");
        }
        if pipeline.join().is_err() {
            error!("Pipeline thread error");
        }
    }
    Ok(())
}

fn run_encoder(tx: UnboundedSender<bytes::Bytes>) {
    let var = VideoVariant {
        mapping: VariantMapping {
            id: Uuid::new_v4(),
            src_index: 0,
            dst_index: 0,
            group_id: 0,
        },
        width: WIDTH as u16,
        height: HEIGHT as u16,
        fps: FPS as u16,
        bitrate: 1_000_000,
        codec: AV_CODEC_ID_H264 as usize,
        profile: AV_PROFILE_H264_MAIN as usize,
        level: 51,
        keyframe_interval: FPS as u16,
        pixel_format: AV_PIX_FMT_YUV420P as u32,
    };
    let mut sws = Scaler::new(var.clone());
    let mut enc = VideoEncoder::new(var.clone());

    let svg_data = std::fs::read("./test.svg").unwrap();
    let tree = usvg::Tree::from_data(&svg_data, &Default::default()).unwrap();
    let mut pixmap = tiny_skia::Pixmap::new(WIDTH as u32, HEIGHT as u32).unwrap();
    let render_ts = tiny_skia::Transform::from_scale(1f32, 1f32);
    resvg::render(&tree, render_ts, &mut pixmap.as_mut());

    let font = include_bytes!("../../SourceCodePro-Regular.ttf") as &[u8];
    let scp = fontdue::Font::from_bytes(font, Default::default()).unwrap();
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    let fonts = &[&scp];

    let start = Instant::now();
    let mut frame_number: u64 = 0;
    loop {
        let stream_time = Duration::from_secs_f64(frame_number as f64 / FPS as f64);
        let real_time = Instant::now().duration_since(start);
        let wait_time = if stream_time > real_time {
            stream_time - real_time
        } else {
            Duration::new(0, 0)
        };
        if !wait_time.is_zero() {
            std::thread::sleep(wait_time);
        }

        frame_number += 1;

        let src_frame = unsafe {
            let src_frame = av_frame_alloc();

            (*src_frame).width = WIDTH;
            (*src_frame).height = HEIGHT;
            (*src_frame).pict_type = AV_PICTURE_TYPE_NONE;
            (*src_frame).key_frame = 1;
            (*src_frame).colorspace = AVCOL_SPC_RGB;
            (*src_frame).format = AV_PIX_FMT_RGBA as libc::c_int;
            (*src_frame).pts = frame_number as i64;
            (*src_frame).duration = 1;
            av_frame_get_buffer(src_frame, 0);

            memcpy(
                (*src_frame).data[0] as *mut libc::c_void,
                pixmap.data().as_ptr() as *const libc::c_void,
                (WIDTH * HEIGHT * 4) as libc::size_t,
            );
            src_frame
        };
        layout.clear();
        layout.append(
            fonts,
            &TextStyle::new(&format!("frame={}", frame_number), 40.0, 0),
        );
        for g in layout.glyphs() {
            let (metrics, bitmap) = scp.rasterize_config_subpixel(g.key);
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
        let pkgs = match sws.process(PipelinePayload::AvFrame(src_frame, AVFrameSource::None(0))) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to scale frame: {}", e);
                return;
            }
        };
        for pkg in pkgs {
            match enc.process(pkg) {
                Ok(pkgs) => {
                    for pkg in pkgs {
                        match pkg {
                            PipelinePayload::AvPacket(pkt, _) => unsafe {
                                let buf = bytes::Bytes::from(slice::from_raw_parts(
                                    (*pkt).data,
                                    (*pkt).size as usize,
                                ));
                                if let Err(e) = tx.send(buf) {
                                    error!("Failed to send test pkt: {}", e);
                                    return;
                                }
                            },
                            _ => {
                                warn!("Unknown payload from encoder: {:?}", pkg);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to encode: {}", e);
                    return;
                }
            }
        }
    }
}
