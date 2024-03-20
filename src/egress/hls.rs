use std::ffi::{CStr, CString};
use std::mem::transmute;
use std::ptr;
use std::ptr::slice_from_raw_parts;

use crate::demux::info::{DemuxStreamInfo, StreamChannelType};
use crate::fraction::Fraction;
use anyhow::Error;
use ffmpeg_sys_next::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
use ffmpeg_sys_next::AVColorSpace::AVCOL_SPC_BT709;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_YUV420P;
use ffmpeg_sys_next::AVSampleFormat::AV_SAMPLE_FMT_FLT;
use ffmpeg_sys_next::{
    av_channel_layout_default, av_dump_format, av_interleaved_write_frame, av_opt_set,
    av_packet_rescale_ts, av_write_frame, avcodec_send_frame, avcodec_send_packet,
    avformat_alloc_output_context2, avformat_new_stream, avformat_write_header, AVChannelLayout,
    AVChannelLayout__bindgen_ty_1, AVCodecContext, AVFormatContext, AVPacket, AVRational,
    AV_CH_LAYOUT_STEREO,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use log::info;
use uuid::{Bytes, Uuid, Variant};

use crate::pipeline::{HLSEgressConfig, PipelinePayload};
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::{VariantStream, VideoVariant};

pub struct HlsEgress {
    /// Pipeline id
    id: Uuid,
    config: HLSEgressConfig,
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<PipelinePayload>,
}

unsafe impl Send for HlsEgress {}
unsafe impl Sync for HlsEgress {}

impl HlsEgress {
    pub fn new(chan_in: UnboundedReceiver<PipelinePayload>, id: Uuid, config: HLSEgressConfig) -> Self {
        Self {
            id,
            config,
            ctx: ptr::null_mut(),
            chan_in,
        }
    }

    unsafe fn setup_muxer(&mut self) -> Result<(), Error> {
        let mut ctx = ptr::null_mut();

        let ret = avformat_alloc_output_context2(
            &mut ctx,
            ptr::null(),
            "hls\0".as_ptr() as *const libc::c_char,
            format!("{}/stream_%v/live.m3u8\0", self.id).as_ptr() as *const libc::c_char,
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        av_opt_set(
            (*ctx).priv_data,
            "hls_segment_filename\0".as_ptr() as *const libc::c_char,
            format!("{}/stream_%v/seg_%05d.ts\0", self.id).as_ptr() as *const libc::c_char,
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

        info!("map_str={}", self.config.stream_map);

        av_opt_set(
            (*ctx).priv_data,
            "var_stream_map\0".as_ptr() as *const libc::c_char,
            format!("{}\0", self.config.stream_map).as_ptr() as *const libc::c_char,
            0,
        );

        for var in &mut self.config.variants {
            match var {
                VariantStream::Video(vs) => {
                    let stream = avformat_new_stream(ctx, ptr::null());
                    if stream == ptr::null_mut() {
                        return Err(Error::msg("Failed to add stream to output"));
                    }

                    // overwrite dst_index to match output stream
                    vs.dst_index = (*stream).index as usize;

                    let params = (*stream).codecpar;
                    (*params).height = vs.height as libc::c_int;
                    (*params).width = vs.width as libc::c_int;
                    (*params).codec_id = transmute(vs.codec as i32);
                    (*params).codec_type = AVMEDIA_TYPE_VIDEO;
                    (*params).format = AV_PIX_FMT_YUV420P as i32;
                    (*params).framerate = AVRational {
                        num: 1,
                        den: vs.fps as libc::c_int,
                    };
                    (*params).bit_rate = vs.bitrate as i64;
                    (*params).color_space = AVCOL_SPC_BT709;
                    (*params).level = vs.level as libc::c_int;
                    (*params).profile = vs.profile as libc::c_int;
                }
                VariantStream::Audio(va) => {
                    let stream = avformat_new_stream(ctx, ptr::null());
                    if stream == ptr::null_mut() {
                        return Err(Error::msg("Failed to add stream to output"));
                    }

                    // overwrite dst_index to match output stream
                    va.dst_index = (*stream).index as usize;

                    let params = (*stream).codecpar;

                    (*params).codec_id = transmute(va.codec as i32);
                    (*params).codec_type = AVMEDIA_TYPE_AUDIO;
                    (*params).format = AV_SAMPLE_FMT_FLT as libc::c_int;
                    (*params).bit_rate = va.bitrate as i64;
                    (*params).sample_rate = va.sample_rate as libc::c_int;
                    (*params).ch_layout = AVChannelLayout {
                        order: AV_CHANNEL_ORDER_NATIVE,
                        nb_channels: 2,
                        u: AVChannelLayout__bindgen_ty_1 {
                            mask: AV_CH_LAYOUT_STEREO,
                        },
                        opaque: ptr::null_mut(),
                    };
                }
                _ => return Err(Error::msg("Invalid config")),
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
        let slice_raw = slice_from_raw_parts((*(*pkt).opaque_ref).data, 16);
        let binding = Bytes::from(*(slice_raw as *const [u8; 16]));
        let variant_id = Uuid::from_bytes_ref(&binding);
        let dst_stream_index = self.config.variants.iter().find_map(|v| match &v {
            VariantStream::Video(vv) => {
                if vv.id.eq(variant_id) {
                    Some(vv.dst_index)
                } else {
                    None
                }
            }
            VariantStream::Audio(va) => {
                if va.id.eq(variant_id) {
                    Some(va.dst_index)
                } else {
                    None
                }
            }
            _ => None,
        });
        if let None = dst_stream_index {
            return Err(Error::msg(format!(
                "No stream found with id={:?}",
                dst_stream_index
            )));
        }

        let stream = *(*self.ctx).streams.add(dst_stream_index.unwrap());
        av_packet_rescale_ts(pkt, (*pkt).time_base, (*stream).time_base);

        (*pkt).stream_index = (*stream).index;

        let ret = av_interleaved_write_frame(self.ctx, pkt);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        Ok(())
    }

    pub fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            match pkg {
                PipelinePayload::AvPacket(pkt) => unsafe {
                    if self.ctx == ptr::null_mut() {
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
