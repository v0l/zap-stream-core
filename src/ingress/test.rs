use std::{ptr, slice};
use std::mem::transmute;
use std::ops::Add;
use std::time::{Duration, SystemTime};

use bytes::BufMut;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, av_frame_free, av_frame_get_buffer, av_packet_alloc,
    av_packet_free, AV_PROFILE_AV1_HIGH, AV_PROFILE_H264_HIGH, AV_PROFILE_H264_MAIN, av_q2d,
    av_write_frame, avcodec_alloc_context3, avcodec_find_encoder, avcodec_get_name,
    avcodec_open2, avcodec_receive_packet, avcodec_send_frame, AVERROR,
    avformat_alloc_context, avformat_alloc_output_context2, AVRational, EAGAIN, sws_alloc_context,
    SWS_BILINEAR, sws_getContext, sws_scale_frame,
};
use ffmpeg_sys_next::AVCodecID::{
    AV_CODEC_ID_H264, AV_CODEC_ID_MPEG1VIDEO, AV_CODEC_ID_MPEG2VIDEO, AV_CODEC_ID_MPEG4,
    AV_CODEC_ID_VP8, AV_CODEC_ID_WMV1,
};
use ffmpeg_sys_next::AVColorSpace::{AVCOL_SPC_BT709, AVCOL_SPC_RGB};
use ffmpeg_sys_next::AVPictureType::{AV_PICTURE_TYPE_I, AV_PICTURE_TYPE_NONE};
use ffmpeg_sys_next::AVPixelFormat::{AV_PIX_FMT_RGB24, AV_PIX_FMT_YUV420P};
use futures_util::StreamExt;
use libc::memcpy;
use log::{error, info, warn};
use rand::random;
use tokio::sync::mpsc::unbounded_channel;

use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;

pub async fn listen(builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    info!("Test pattern enabled");

    const WIDTH: libc::c_int = 1280;
    const HEIGHT: libc::c_int = 720;
    const TBN: libc::c_int = 30;

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
                (*enc_ctx).framerate = AVRational { num: 30, den: 1 };
                (*enc_ctx).gop_size = 30;
                (*enc_ctx).level = 40;
                (*enc_ctx).profile = AV_PROFILE_H264_MAIN;
                (*enc_ctx).time_base = AVRational { num: 1, den: TBN };
                (*enc_ctx).pkt_timebase = (*enc_ctx).time_base;

                avcodec_open2(enc_ctx, codec, ptr::null_mut());

                let src_frame = av_frame_alloc();
                (*src_frame).width = WIDTH;
                (*src_frame).height = HEIGHT;
                (*src_frame).pict_type = AV_PICTURE_TYPE_NONE;
                (*src_frame).key_frame = 1;
                (*src_frame).colorspace = AVCOL_SPC_RGB;
                (*src_frame).format = AV_PIX_FMT_RGB24 as libc::c_int;
                (*src_frame).time_base = (*enc_ctx).time_base;
                av_frame_get_buffer(src_frame, 0);

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
                let render_ts = tiny_skia::Transform::from_scale(0.5, 0.5);
                resvg::render(&tree, render_ts, &mut pixmap.as_mut());

                for x in 0..WIDTH as u32 {
                    for y in 0..HEIGHT as u32 {
                        if let Some(px) = pixmap.pixel(x, y) {
                            let offset = 3 * x + y * (*src_frame).linesize[0] as u32;
                            let pixel = (*src_frame).data[0].add(offset as usize);
                            *pixel.offset(0) = px.red();
                            *pixel.offset(1) = px.green();
                            *pixel.offset(2) = px.blue();
                        }
                    }
                }

                let mut frame_number: u64 = 0;
                let start = SystemTime::now();
                loop {
                    frame_number += 1;
                    (*src_frame).pts = (TBN as u64 * frame_number) as i64;

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
                        for z in 0..(buf.len() as f32 / 1024.0).ceil() as usize {
                            if let Err(e) = tx.send(buf.slice(z..(z + 1024).min(buf.len()))) {
                                error!("Failed to write data {}", e);
                                break;
                            }
                        }
                    }

                    let stream_time = Duration::from_secs_f64(
                        frame_number as libc::c_double * av_q2d((*enc_ctx).time_base),
                    );
                    let real_time = SystemTime::now().duration_since(start).unwrap();
                    let wait_time = stream_time - real_time;
                    std::thread::sleep(wait_time);
                }
            }
        }
    });
    Ok(())
}
