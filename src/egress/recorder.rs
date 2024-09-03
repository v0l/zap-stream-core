use std::collections::{HashSet, VecDeque};
use std::{fs, ptr};

use anyhow::Error;
use ffmpeg_sys_next::{
    av_dump_format, av_interleaved_write_frame, av_opt_set, avformat_alloc_output_context2,
    avformat_free_context, avio_open2, AVFormatContext, AVPacket, AVIO_FLAG_WRITE,
};
use ffmpeg_sys_next::{
    avcodec_parameters_from_context, avformat_write_header, AVFMT_GLOBALHEADER,
    AV_CODEC_FLAG_GLOBAL_HEADER,
};
use log::info;
use uuid::Uuid;

use crate::egress::{map_variants_to_streams, EgressConfig};
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::return_ffmpeg_error;
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::{find_stream, StreamMapping, VariantStream};

pub struct RecorderEgress {
    id: Uuid,
    config: EgressConfig,
    variants: Vec<VariantStream>,
    ctx: *mut AVFormatContext,
    stream_init: HashSet<Uuid>,
    init: bool,
    packet_buffer: VecDeque<PipelinePayload>,
}

unsafe impl Send for RecorderEgress {}

unsafe impl Sync for RecorderEgress {}

impl Drop for RecorderEgress {
    fn drop(&mut self) {
        unsafe {
            avformat_free_context(self.ctx);
            self.ctx = ptr::null_mut();
        }
    }
}

impl RecorderEgress {
    pub fn new(id: Uuid, config: EgressConfig, variants: Vec<VariantStream>) -> Self {
        let filtered_vars: Vec<VariantStream> = config
            .variants
            .iter()
            .filter_map(|x| variants.iter().find(|y| y.id() == *x))
            .cloned()
            .collect();
        Self {
            id,
            config,
            variants: filtered_vars,
            ctx: ptr::null_mut(),
            stream_init: HashSet::new(),
            init: false,
            packet_buffer: VecDeque::new(),
        }
    }

    unsafe fn setup_muxer(&mut self) -> Result<(), Error> {
        let base = format!("{}/{}", self.config.out_dir, self.id);

        let out_file = format!("{}/recording.mp4\0", base);
        fs::create_dir_all(base.clone())?;

        let mut ctx = ptr::null_mut();
        let ret = avformat_alloc_output_context2(
            &mut ctx,
            ptr::null_mut(),
            ptr::null_mut(),
            out_file.as_ptr() as *const libc::c_char,
        );
        return_ffmpeg_error!(ret);
        map_variants_to_streams(ctx, &self.variants)?;

        if (*(*ctx).oformat).flags & AVFMT_GLOBALHEADER != 0 {
            (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as libc::c_int;
        }
        av_opt_set(
            (*ctx).priv_data,
            "movflags\0".as_ptr() as *const libc::c_char,
            "+dash+delay_moov+skip_sidx+skip_trailer\0".as_ptr() as *const libc::c_char,
            0,
        );
        self.ctx = ctx;
        Ok(())
    }

    unsafe fn open_muxer(&mut self) -> Result<bool, Error> {
        if !self.init && self.stream_init.len() == self.config.variants.len() {
            let ret = avio_open2(
                &mut (*self.ctx).pb,
                (*self.ctx).url,
                AVIO_FLAG_WRITE,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            return_ffmpeg_error!(ret);

            av_dump_format(self.ctx, 0, ptr::null(), 1);
            let ret = avformat_write_header(self.ctx, ptr::null_mut());
            return_ffmpeg_error!(ret);

            self.init = true;
            Ok(true)
        } else {
            Ok(self.init)
        }
    }

    unsafe fn process_pkt(&mut self, pkt: *mut AVPacket) -> Result<(), Error> {
        //dump_pkt_info(pkt);
        let ret = av_interleaved_write_frame(self.ctx, pkt);
        return_ffmpeg_error!(ret);
        Ok(())
    }
}

impl PipelineProcessor for RecorderEgress {
    fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error> {
        match pkg {
            PipelinePayload::AvPacket(pkt, ref src) => unsafe {
                if self.open_muxer()? {
                    while let Some(pkt) = self.packet_buffer.pop_front() {
                        match pkt {
                            PipelinePayload::AvPacket(pkt, ref src) => {
                                self.process_pkt(pkt)?;
                            }
                            _ => return Err(Error::msg("")),
                        }
                    }
                    self.process_pkt(pkt)?;
                } else {
                    self.packet_buffer.push_back(pkg);
                }
            },
            PipelinePayload::EncoderInfo(ref var, ctx) => unsafe {
                if self.ctx.is_null() {
                    self.setup_muxer()?;
                }
                if !self.stream_init.contains(var) {
                    let my_var = find_stream(&self.variants, var)?;
                    let out_stream = *(*self.ctx).streams.add(my_var.dst_index());
                    avcodec_parameters_from_context((*out_stream).codecpar, ctx);
                    (*(*out_stream).codecpar).codec_tag = 0;

                    self.stream_init.insert(*var);
                    info!("Setup encoder info: {}", my_var);
                }
            },
            _ => return Err(Error::msg("Payload not supported")),
        }
        // Muxer never returns anything
        Ok(vec![])
    }
}
