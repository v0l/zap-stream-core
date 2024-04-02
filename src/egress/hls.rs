use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_sys_next::{
    av_dump_format, av_get_sample_fmt, av_interleaved_write_frame, av_opt_set,
    avcodec_find_encoder, avcodec_parameters_from_context, avformat_alloc_output_context2,
    avformat_free_context, avformat_new_stream, avformat_write_header, AVChannelLayout,
    AVChannelLayout__bindgen_ty_1, AVCodecContext, AVFormatContext, AVPacket, AVRational,
    AV_CH_LAYOUT_STEREO,
};
use itertools::Itertools;
use log::info;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::egress::{map_variants_to_streams, EgressConfig, update_pkt_for_muxer, get_pkt_variant};
use crate::encode::dump_pkt_info;
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::{get_ffmpeg_error_msg, id_ref_to_uuid};
use crate::variant::{VariantStream, VariantStreamType};

pub struct HlsEgress {
    id: Uuid,
    config: EgressConfig,
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<PipelinePayload>,
}

unsafe impl Send for HlsEgress {}

unsafe impl Sync for HlsEgress {}

impl Drop for HlsEgress {
    fn drop(&mut self) {
        unsafe {
            avformat_free_context(self.ctx);
            self.ctx = ptr::null_mut();
        }
    }
}

impl HlsEgress {
    pub fn new(
        chan_in: UnboundedReceiver<PipelinePayload>,
        id: Uuid,
        config: EgressConfig,
    ) -> Self {
        Self {
            id,
            config,
            ctx: ptr::null_mut(),
            chan_in,
        }
    }

    unsafe fn setup_muxer(&mut self) -> Result<(), Error> {
        let mut ctx = ptr::null_mut();

        let base = format!("{}/{}", self.config.out_dir, self.id);

        let ret = avformat_alloc_output_context2(
            &mut ctx,
            ptr::null(),
            "hls\0".as_ptr() as *const libc::c_char,
            format!("{}/%v/live.m3u8\0", base).as_ptr() as *const libc::c_char,
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        av_opt_set(
            (*ctx).priv_data,
            "hls_segment_filename\0".as_ptr() as *const libc::c_char,
            format!("{}/%v/%05d.ts\0", base).as_ptr() as *const libc::c_char,
            0,
        );

        av_opt_set(
            (*ctx).priv_data,
            "master_pl_name\0".as_ptr() as *const libc::c_char,
            "live.m3u8\0".as_ptr() as *const libc::c_char,
            0,
        );

        av_opt_set(
            (*ctx).priv_data,
            "master_pl_publish_rate\0".as_ptr() as *const libc::c_char,
            "10\0".as_ptr() as *const libc::c_char,
            0,
        );

        if let Some(first_video_track) = self.config.variants.iter().find_map(|v| {
            if let VariantStream::Video(vv) = v {
                Some(vv)
            } else {
                None
            }
        }) {
            av_opt_set(
                (*ctx).priv_data,
                "hls_time\0".as_ptr() as *const libc::c_char,
                format!("{}\0", first_video_track.keyframe_interval).as_ptr()
                    as *const libc::c_char,
                0,
            );
        }

        av_opt_set(
            (*ctx).priv_data,
            "hls_flags\0".as_ptr() as *const libc::c_char,
            "delete_segments\0".as_ptr() as *const libc::c_char,
            0,
        );

        // configure mapping
        let mut stream_map: HashMap<usize, Vec<String>> = HashMap::new();
        for var in &self.config.variants {
            let cfg = match var {
                VariantStream::Video(vx) => format!("v:{}", vx.dst_index),
                VariantStream::Audio(ax) => format!("a:{}", ax.dst_index),
            };
            if let Some(out_stream) = stream_map.get_mut(&var.dst_index()) {
                out_stream.push(cfg);
            } else {
                stream_map.insert(var.dst_index(), vec![cfg]);
            }
        }
        let stream_map = stream_map.values().map(|v| v.join(",")).join(" ");

        info!("map_str={}", stream_map);

        av_opt_set(
            (*ctx).priv_data,
            "var_stream_map\0".as_ptr() as *const libc::c_char,
            format!("{}\0", stream_map).as_ptr() as *const libc::c_char,
            0,
        );

        map_variants_to_streams(ctx, &mut self.config.variants)?;

        let ret = avformat_write_header(ctx, ptr::null_mut());
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        self.ctx = ctx;
        Ok(())
    }

    unsafe fn process_pkt(&mut self, pkt: *mut AVPacket) -> Result<(), Error> {
        let variant = get_pkt_variant(&self.config.variants, pkt)?;
        update_pkt_for_muxer(self.ctx, pkt, &variant);

        //dump_pkt_info(pkt);
        let ret = av_interleaved_write_frame(self.ctx, pkt);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        Ok(())
    }
}

impl PipelineProcessor for HlsEgress {
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            match pkg {
                PipelinePayload::AvPacket(_, pkt) => unsafe {
                    if self.ctx.is_null() {
                        self.setup_muxer()?;
                    }
                    self.process_pkt(pkt)?;
                },
                _ => return Err(Error::msg("Payload not supported")),
            }
        }
        Ok(())
    }
}
