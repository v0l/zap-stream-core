use std::{ptr, slice};
use std::mem::transmute;
use std::ops::Add;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use bytes::BufMut;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, av_frame_free, av_frame_get_buffer, av_packet_alloc,
    av_packet_free, AV_PROFILE_AV1_HIGH, AV_PROFILE_H264_HIGH, av_q2d, av_write_frame,
    avcodec_alloc_context3, avcodec_find_encoder, avcodec_get_name, avcodec_open2,
    avcodec_receive_packet, avcodec_send_frame, AVERROR, avformat_alloc_context,
    avformat_alloc_output_context2, AVRational, EAGAIN, sws_alloc_context, SWS_BILINEAR, sws_getContext,
    sws_scale_frame,
};
use ffmpeg_sys_next::AVCodecID::{
    AV_CODEC_ID_H264, AV_CODEC_ID_MPEG1VIDEO, AV_CODEC_ID_MPEG2VIDEO, AV_CODEC_ID_MPEG4,
    AV_CODEC_ID_VP8, AV_CODEC_ID_WMV1,
};
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_RGB;
use ffmpeg_sys_next::AVPictureType::{AV_PICTURE_TYPE_I, AV_PICTURE_TYPE_NONE};
use ffmpeg_sys_next::AVPixelFormat::{AV_PIX_FMT_RGB24, AV_PIX_FMT_YUV420P};
use futures_util::StreamExt;
use libc::memcpy;
use log::{error, info, warn};
use rand::random;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::unbounded_channel;

use crate::ingress::ConnectionInfo;
use crate::pipeline::builder::PipelineBuilder;

pub async fn listen(path: PathBuf, builder: PipelineBuilder) -> Result<(), anyhow::Error> {
    info!("Sending file {}", path.to_str().unwrap());

    tokio::spawn(async move {
        let (tx, rx) = unbounded_channel();
        let info = ConnectionInfo {
            ip_addr: "".to_owned(),
            endpoint: "file-input".to_owned(),
        };

        if let Ok(mut pl) = builder.build_for(info, rx).await {
            std::thread::spawn(move || loop {
                if let Err(e) = pl.run() {
                    warn!("Pipeline error: {}", e.backtrace());
                    break;
                }
            });

            if let Ok(mut stream) = tokio::fs::File::open(path).await {
                let mut buf = [0u8; 1500];
                loop {
                    if let Ok(r) = stream.read(&mut buf).await {
                        if r > 0 {
                            tx.send(bytes::Bytes::copy_from_slice(&buf[..r])).unwrap();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                info!("EOF");
            }
        }
    });
    Ok(())
}
