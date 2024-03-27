use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{AV_CH_LAYOUT_STEREO, av_dump_format, av_get_sample_fmt, av_interleaved_write_frame, av_opt_set, AVChannelLayout, AVChannelLayout__bindgen_ty_1, avcodec_find_encoder, avcodec_parameters_from_context, AVCodecContext, avformat_alloc_output_context2, avformat_free_context, avformat_new_stream, avformat_write_header, AVFormatContext, AVPacket, AVRational};
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
use itertools::Itertools;
use log::info;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::pipeline::PipelinePayload;
use crate::utils::{get_ffmpeg_error_msg, id_ref_to_uuid};
use crate::variant::VariantStream;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HLSEgressConfig {
    pub out_dir: String,
    pub variants: Vec<VariantStream>,
}

impl Display for HLSEgressConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HLS: out_dir={}", self.out_dir)?;
        if !self.variants.is_empty() {
            write!(f, "\n\tStreams: ")?;
            for v in &self.variants {
                write!(f, "\n\t\t{}", v)?;
            }
        }
        Ok(())
    }
}

pub struct HlsEgress {
    id: Uuid,
    config: HLSEgressConfig,
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<PipelinePayload>,
    stream_init: HashSet<i32>,
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
        config: HLSEgressConfig,
    ) -> Self {
        Self {
            id,
            config,
            ctx: ptr::null_mut(),
            chan_in,
            stream_init: HashSet::new(),
        }
    }

    unsafe fn setup_muxer(&mut self) -> Result<(), Error> {
        let mut ctx = ptr::null_mut();

        let base = format!("{}/{}", self.config.out_dir, self.id);

        let ret = avformat_alloc_output_context2(
            &mut ctx,
            ptr::null(),
            "hls\0".as_ptr() as *const libc::c_char,
            format!("{}/stream_%v/live.m3u8\0", base).as_ptr() as *const libc::c_char,
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        av_opt_set(
            (*ctx).priv_data,
            "hls_segment_filename\0".as_ptr() as *const libc::c_char,
            format!("{}/stream_%v/seg_%05d.ts\0", base).as_ptr() as *const libc::c_char,
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
        let stream_map = stream_map
            .values()
            .map(|v| v.join(","))
            .join(" ");

        info!("map_str={}", stream_map);

        av_opt_set(
            (*ctx).priv_data,
            "var_stream_map\0".as_ptr() as *const libc::c_char,
            format!("{}\0", stream_map).as_ptr() as *const libc::c_char,
            0,
        );

        for var in &mut self.config.variants {
            match var {
                VariantStream::Video(vs) => {
                    let stream = avformat_new_stream(ctx, ptr::null());
                    if stream.is_null() {
                        return Err(Error::msg("Failed to add stream to output"));
                    }

                    // overwrite dst_index to match output stream
                    vs.dst_index = (*stream).index as usize;
                    vs.to_stream(stream);
                    vs.to_codec_params((*stream).codecpar);
                }
                VariantStream::Audio(va) => {
                    let stream = avformat_new_stream(ctx, ptr::null());
                    if stream.is_null() {
                        return Err(Error::msg("Failed to add stream to output"));
                    }

                    // overwrite dst_index to match output stream
                    va.dst_index = (*stream).index as usize;
                    va.to_stream(stream);
                    va.to_codec_params((*stream).codecpar);
                }
            }
        }

        av_dump_format(ctx, 0, ptr::null(), 1);

        let ret = avformat_write_header(ctx, ptr::null_mut());
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        self.ctx = ctx;
        Ok(())
    }

    unsafe fn process_pkt(&mut self, pkt: *mut AVPacket) -> Result<(), Error> {
        let variant_id = id_ref_to_uuid((*pkt).opaque_ref)?;
        let variant = self.config.variants.iter().find(|v| v.id() == variant_id);
        if variant.is_none() {
            return Err(Error::msg(format!(
                "No stream found with id={:?}",
                variant_id
            )));
        }

        let stream = *(*self.ctx).streams.add(variant.unwrap().dst_index());
        let idx = (*stream).index;
        (*pkt).stream_index = idx;
        if !self.stream_init.contains(&idx) {
            let encoder = (*pkt).opaque as *mut AVCodecContext;
            avcodec_parameters_from_context((*stream).codecpar, encoder);
            self.stream_init.insert(idx);
        }

        let ret = av_interleaved_write_frame(self.ctx, pkt);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        Ok(())
    }

    pub fn process(&mut self) -> Result<(), Error> {
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
