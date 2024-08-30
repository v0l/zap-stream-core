use std::{ptr, slice};
use std::mem::transmute;
use std::ops::Add;
use std::time::{Duration, SystemTime};

use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, av_frame_free, av_frame_get_buffer, av_packet_alloc,
    av_packet_free, AV_PROFILE_H264_MAIN, av_q2d, avcodec_alloc_context3, avcodec_find_encoder,
    avcodec_open2, avcodec_receive_packet, avcodec_send_frame, AVERROR, AVRational,
    EAGAIN, SWS_BILINEAR, sws_getContext, sws_scale_frame,
};
use ffmpeg_sys_next::AVCodecID::AV_CODEC_ID_H264;
use ffmpeg_sys_next::AVColorSpace::{AVCOL_SPC_BT709, AVCOL_SPC_RGB};
use ffmpeg_sys_next::AVPictureType::AV_PICTURE_TYPE_NONE;
use ffmpeg_sys_next::AVPixelFormat::{AV_PIX_FMT_RGB24, AV_PIX_FMT_RGBA, AV_PIX_FMT_YUV420P};
use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
use libc::memcpy;
use log::{error, info};
use tokio::sync::mpsc::unbounded_channel;
use usvg::{Font, Node};

use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;

pub async fn listen(builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    info!("Test pattern enabled");

    const WIDTH: libc::c_int = 1920;
    const HEIGHT: libc::c_int = 1080;
    const FPS: libc::c_int = 25;

    tokio::spawn(async move {
        let (tx, rx) = unbounded_channel();
        let info = ConnectionInfo {
            ip_addr: "".to_owned(),
            endpoint: "test-pattern".to_owned(),
        };

        if let Ok(mut pl) = builder.build_for(info, rx).await {
            std::thread::spawn(move || loop {
                if let Err(e) = pl.run() {
                    error!("Pipeline error: {}\n{}", e, e.backtrace());
                    break;
                }
            });
            unsafe {
                let codec = avcodec_find_encoder(AV_CODEC_ID_H264);
                let enc_ctx = avcodec_alloc_context3(codec);
                (*enc_ctx).width = WIDTH;
                (*enc_ctx).height = HEIGHT;
                (*enc_ctx).pix_fmt = AV_PIX_FMT_YUV420P;
                (*enc_ctx).colorspace = AVCOL_SPC_BT709;
                (*enc_ctx).bit_rate = 1_000_000;
                (*enc_ctx).framerate = AVRational { num: FPS, den: 1 };
                (*enc_ctx).gop_size = 30;
                (*enc_ctx).level = 40;
                (*enc_ctx).profile = AV_PROFILE_H264_MAIN;
                (*enc_ctx).time_base = AVRational { num: 1, den: FPS };
                (*enc_ctx).pkt_timebase = (*enc_ctx).time_base;

                avcodec_open2(enc_ctx, codec, ptr::null_mut());

                let src_frame = av_frame_alloc();
                (*src_frame).width = WIDTH;
                (*src_frame).height = HEIGHT;
                (*src_frame).pict_type = AV_PICTURE_TYPE_NONE;
                (*src_frame).key_frame = 1;
                (*src_frame).colorspace = AVCOL_SPC_RGB;
                (*src_frame).format = AV_PIX_FMT_RGBA as libc::c_int;
                (*src_frame).time_base = (*enc_ctx).time_base;
                av_frame_get_buffer(src_frame, 1);

                let sws = sws_getContext(
                    WIDTH as libc::c_int,
                    HEIGHT as libc::c_int,
                    transmute((*src_frame).format),
                    WIDTH as libc::c_int,
                    HEIGHT as libc::c_int,
                    (*enc_ctx).pix_fmt,
                    SWS_BILINEAR,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                let svg_data = std::fs::read("./test.svg").unwrap();
                let tree = usvg::Tree::from_data(&svg_data, &Default::default()).unwrap();
                let mut pixmap = tiny_skia::Pixmap::new(WIDTH as u32, HEIGHT as u32).unwrap();
                let render_ts = tiny_skia::Transform::from_scale(1f32, 1f32);
                resvg::render(&tree, render_ts, &mut pixmap.as_mut());

                let font = include_bytes!("../../SourceCodePro-Regular.ttf") as &[u8];
                let scp = fontdue::Font::from_bytes(font, Default::default()).unwrap();
                let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
                let fonts = &[&scp];

                let mut frame_number: u64 = 0;
                loop {
                    frame_number += 1;
                    (*src_frame).pts = frame_number as i64;
                    (*src_frame).duration = 1;

                    memcpy(
                        (*src_frame).data[0] as *mut libc::c_void,
                        pixmap.data().as_ptr() as *const libc::c_void,
                        (WIDTH * HEIGHT * 4) as libc::size_t,
                    );

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
                                let offset_dst =
                                    4 * dst_x + dst_y * (*src_frame).linesize[0] as usize;
                                let pixel_dst = (*src_frame).data[0].add(offset_dst);
                                *pixel_dst.offset(0) = bitmap[offset_src];
                                *pixel_dst.offset(1) = bitmap[offset_src + 1];
                                *pixel_dst.offset(2) = bitmap[offset_src + 2];
                            }
                        }
                    }

                    let mut dst_frame = av_frame_alloc();
                    av_frame_copy_props(dst_frame, src_frame);
                    sws_scale_frame(sws, dst_frame, src_frame);

                    // encode
                    let mut ret = avcodec_send_frame(enc_ctx, dst_frame);
                    av_frame_free(&mut dst_frame);

                    while ret > 0 || ret == AVERROR(libc::EAGAIN) {
                        let mut av_pkt = av_packet_alloc();
                        ret = avcodec_receive_packet(enc_ctx, av_pkt);
                        if ret != 0 {
                            if ret == AVERROR(EAGAIN) {
                                av_packet_free(&mut av_pkt);
                                break;
                            }
                            error!("Encoder failed: {}", ret);
                            break;
                        }

                        let buf = bytes::Bytes::from(slice::from_raw_parts(
                            (*av_pkt).data,
                            (*av_pkt).size as usize,
                        ));
                        if let Err(e) = tx.send(buf) {
                            error!("Failed to send test pkt: {}", e);
                            return;
                        }
                    }
                }
            }
        }
    });
    Ok(())
}
