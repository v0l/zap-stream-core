use std::collections::{HashMap, HashSet, VecDeque};
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_dump_format, av_interleaved_write_frame, av_opt_set, av_packet_clone, av_packet_copy_props,
    avcodec_parameters_from_context, avformat_alloc_output_context2, avformat_free_context,
    avformat_write_header, AVFormatContext, AVPacket,
};
use itertools::Itertools;
use log::info;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::egress::{EgressConfig, map_variants_to_streams};
use crate::pipeline::{AVPacketSource, PipelinePayload, PipelineProcessor};
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::{VariantStream, VariantStreamType};

pub struct HlsEgress {
    id: Uuid,
    config: EgressConfig,
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<PipelinePayload>,
    stream_init: HashSet<Uuid>,
    init: bool,
    packet_buffer: VecDeque<PipelinePayload>,
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
            init: false,
            stream_init: HashSet::new(),
            packet_buffer: VecDeque::new(),
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

        self.ctx = ctx;
        Ok(())
    }

    unsafe fn process_pkt_internal(
        &mut self,
        pkt: *mut AVPacket,
        src: &AVPacketSource,
    ) -> Result<(), Error> {
        let variant = match src {
            AVPacketSource::Encoder(v) => self
                .config
                .variants
                .iter()
                .find(|x| x.id() == *v)
                .ok_or(Error::msg("Variant does not exist"))?,
            _ => return Err(Error::msg(format!("Cannot mux packet from {:?}", src))),
        };
        (*pkt).stream_index = variant.dst_index() as libc::c_int;

        //dump_pkt_info(pkt);
        let ret = av_interleaved_write_frame(self.ctx, pkt);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }
        Ok(())
    }

    unsafe fn process_pkt(
        &mut self,
        pkt: *mut AVPacket,
        src: &AVPacketSource,
    ) -> Result<(), Error> {
        let variant = match &src {
            AVPacketSource::Encoder(v) => v,
            _ => return Err(Error::msg(format!("Cannot mux packet from {:?}", src))),
        };
        if !self.init {
            let pkt_clone = av_packet_clone(pkt);
            av_packet_copy_props(pkt_clone, pkt);
            self.packet_buffer.push_back(PipelinePayload::AvPacket(
                pkt_clone,
                AVPacketSource::Muxer(*variant),
            ));
        }

        if !self.init && self.stream_init.len() == self.config.variants.len() {
            let ret = avformat_write_header(self.ctx, ptr::null_mut());
            if ret < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg(ret)));
            }

            av_dump_format(self.ctx, 0, ptr::null(), 1);
            self.init = true;
            // push in pkts from buffer
            while let Some(pkt) = self.packet_buffer.pop_front() {
                match pkt {
                    PipelinePayload::AvPacket(pkt, ref src) => {
                        self.process_pkt_internal(pkt, src)?;
                    }
                    _ => return Err(Error::msg("")),
                }
            }
            return Ok(());
        } else if !self.init {
            return Ok(());
        }

        self.process_pkt_internal(pkt, src)
    }
}

impl PipelineProcessor for HlsEgress {
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            match pkg {
                PipelinePayload::AvPacket(pkt, ref src) => unsafe {
                    self.process_pkt(pkt, src)?;
                },
                PipelinePayload::EncoderInfo(ref var, ctx) => unsafe {
                    if self.ctx.is_null() {
                        self.setup_muxer()?;
                    }
                    if !self.stream_init.contains(var) {
                        let variant = self
                            .config
                            .variants
                            .iter()
                            .find(|x| x.id() == *var)
                            .ok_or(Error::msg("Variant does not exist"))?;
                        let out_stream = *(*self.ctx).streams.add(variant.dst_index());
                        avcodec_parameters_from_context((*out_stream).codecpar, ctx);
                        self.stream_init.insert(*var);
                    }
                },
                _ => return Err(Error::msg(format!("Payload not supported: {:?}", pkg))),
            }
        }
        Ok(())
    }
}
