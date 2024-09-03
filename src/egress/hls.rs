use std::collections::{HashSet, VecDeque};
use std::fmt::Display;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_dump_format, av_interleaved_write_frame, av_opt_set, av_packet_rescale_ts,
    avcodec_parameters_copy, avcodec_parameters_from_context, avformat_alloc_output_context2,
    avformat_free_context, avformat_write_header, AVFormatContext, AVPacket, AVStream,
};
use itertools::Itertools;
use log::info;
use uuid::Uuid;

use crate::egress::{map_variants_to_streams, EgressConfig};
use crate::encode::dump_pkt_info;
use crate::pipeline::{AVPacketSource, PipelinePayload, PipelineProcessor};
use crate::return_ffmpeg_error;
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::{find_stream, StreamMapping, VariantStream};

pub struct HlsEgress {
    id: Uuid,
    config: EgressConfig,
    variants: Vec<VariantStream>,
    ctx: *mut AVFormatContext,
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

enum HlsMapEntry {
    Video(usize),
    Audio(usize),
    Subtitle(usize),
}

impl Display for HlsMapEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HlsMapEntry::Video(i) => write!(f, "v:{}", i),
            HlsMapEntry::Audio(i) => write!(f, "a:{}", i),
            HlsMapEntry::Subtitle(i) => write!(f, "s:{}", i),
        }
    }
}

struct HlsStream {
    name: String,
    entries: Vec<HlsMapEntry>,
}

impl Display for HlsStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},name:{}", self.entries.iter().join(","), self.name)
    }
}

impl HlsEgress {
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
            init: false,
            stream_init: HashSet::new(),
            packet_buffer: VecDeque::new(),
        }
    }

    pub(crate) fn setup_muxer(&mut self) -> Result<(), Error> {
        unsafe {
            let mut ctx = ptr::null_mut();

            let base = format!("{}/{}", self.config.out_dir, self.id);

            let ret = avformat_alloc_output_context2(
                &mut ctx,
                ptr::null(),
                "hls\0".as_ptr() as *const libc::c_char,
                format!("{}/%v/live.m3u8\0", base).as_ptr() as *const libc::c_char,
            );
            return_ffmpeg_error!(ret);
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

            if let Some(first_video_track) = self.variants.iter().find_map(|v| {
                if let VariantStream::Video(vv) = v {
                    Some(vv)
                } else {
                    None
                }
            }) {
                av_opt_set(
                    (*ctx).priv_data,
                    "hls_time\0".as_ptr() as *const libc::c_char,
                    format!(
                        "{}\0",
                        first_video_track.keyframe_interval / first_video_track.fps
                    )
                    .as_ptr() as *const libc::c_char,
                    0,
                );
            }

            av_opt_set(
                (*ctx).priv_data,
                "hls_flags\0".as_ptr() as *const libc::c_char,
                "delete_segments\0".as_ptr() as *const libc::c_char,
                0,
            );

            map_variants_to_streams(ctx, &self.variants)?;
            self.ctx = ctx;
            Ok(())
        }
    }

    unsafe fn setup_hls_mapping(&mut self) -> Result<(), Error> {
        if self.ctx.is_null() {
            return Err(Error::msg("Context not setup"));
        }

        // configure mapping
        let mut stream_map = Vec::new();
        for (g, vars) in &self
            .variants
            .iter()
            .sorted_by(|a, b| a.group_id().cmp(&b.group_id()))
            .group_by(|x| x.group_id())
        {
            let mut group = HlsStream {
                name: format!("stream_{}", g),
                entries: Vec::new(),
            };
            for var in vars {
                let n = Self::get_as_nth_stream_type(self.ctx, var);
                match var {
                    VariantStream::Video(_) => group.entries.push(HlsMapEntry::Video(n)),
                    VariantStream::Audio(_) => group.entries.push(HlsMapEntry::Audio(n)),
                    VariantStream::CopyVideo(_) => group.entries.push(HlsMapEntry::Video(n)),
                    VariantStream::CopyAudio(_) => group.entries.push(HlsMapEntry::Audio(n)),
                };
            }
            stream_map.push(group);
        }
        let stream_map = stream_map.iter().join(" ");

        info!("map_str={}", stream_map);

        av_opt_set(
            (*self.ctx).priv_data,
            "var_stream_map\0".as_ptr() as *const libc::c_char,
            format!("{}\0", stream_map).as_ptr() as *const libc::c_char,
            0,
        );

        av_dump_format(self.ctx, 0, ptr::null(), 1);
        Ok(())
    }
    unsafe fn process_av_packet_internal(
        &mut self,
        pkt: *mut AVPacket,
        src: &AVPacketSource,
    ) -> Result<(), Error> {
        let variant = match src {
            AVPacketSource::Encoder(v) => find_stream(&self.variants, v)?,
            AVPacketSource::Demuxer(v) => {
                let var = self
                    .variants
                    .iter()
                    .find(|x| x.src_index() == (*(*v)).index as usize)
                    .ok_or(Error::msg("Demuxer packet didn't match any variant"))?;
                let dst_stream = Self::get_dst_stream(self.ctx, var.dst_index());
                av_packet_rescale_ts(pkt, (*(*v)).time_base, (*dst_stream).time_base);
                var
            }
        };
        (*pkt).stream_index = variant.dst_index() as libc::c_int;

        let ret = av_interleaved_write_frame(self.ctx, pkt);
        return_ffmpeg_error!(ret);
        Ok(())
    }

    fn process_payload_internal(&mut self, pkg: PipelinePayload) -> Result<(), Error> {
        if let PipelinePayload::AvPacket(p, ref s) = pkg {
            unsafe {
                self.process_av_packet_internal(p, s)?;
            }
        }
        Ok(())
    }

    unsafe fn process_payload(&mut self, pkg: PipelinePayload) -> Result<(), Error> {
        if !self.init && self.stream_init.len() == self.config.variants.len() {
            self.setup_hls_mapping()?;

            let ret = avformat_write_header(self.ctx, ptr::null_mut());
            return_ffmpeg_error!(ret);

            self.init = true;
            // dequeue buffer
            while let Some(pkt) = self.packet_buffer.pop_front() {
                self.process_payload_internal(pkt)?;
            }
            return Ok(());
        } else if !self.init {
            self.packet_buffer.push_back(pkg);
            return Ok(());
        }

        self.process_payload_internal(pkg)
    }

    unsafe fn get_dst_stream(ctx: *const AVFormatContext, idx: usize) -> *mut AVStream {
        for x in 0..(*ctx).nb_streams {
            let stream = *(*ctx).streams.add(x as usize);
            if (*stream).index as usize == idx {
                return stream;
            }
        }
        panic!("Stream index not found in output")
    }

    unsafe fn get_as_nth_stream_type(ctx: *const AVFormatContext, var: &VariantStream) -> usize {
        let stream = Self::get_dst_stream(ctx, var.dst_index());
        let mut ctr = 0;
        for x in 0..(*ctx).nb_streams {
            let stream_x = *(*ctx).streams.add(x as usize);
            if (*(*stream).codecpar).codec_type == (*(*stream_x).codecpar).codec_type {
                if (*stream_x).index == (*stream).index {
                    break;
                }
                ctr += 1;
            }
        }
        ctr
    }
}

impl PipelineProcessor for HlsEgress {
    fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error> {
        match pkg {
            PipelinePayload::AvPacket(_, _) => unsafe {
                self.process_payload(pkg)?;
            },
            PipelinePayload::SourceInfo(ref d) => unsafe {
                for var in &self.variants {
                    match var {
                        VariantStream::CopyVideo(cv) => {
                            let src = *(*d.ctx).streams.add(cv.src_index);
                            let dst = Self::get_dst_stream(self.ctx, cv.dst_index);
                            let ret = avcodec_parameters_copy((*dst).codecpar, (*src).codecpar);
                            return_ffmpeg_error!(ret);
                            self.stream_init.insert(var.id());
                        }
                        VariantStream::CopyAudio(ca) => {
                            let src = *(*d.ctx).streams.add(ca.src_index);
                            let dst = Self::get_dst_stream(self.ctx, ca.dst_index);
                            let ret = avcodec_parameters_copy((*dst).codecpar, (*src).codecpar);
                            return_ffmpeg_error!(ret);
                            self.stream_init.insert(var.id());
                        }
                        _ => {}
                    }
                }
            },
            PipelinePayload::EncoderInfo(ref var, ctx) => unsafe {
                if let Some(my_var) = self.variants.iter().find(|x| x.id() == *var) {
                    if !self.stream_init.contains(var) {
                        let out_stream = Self::get_dst_stream(self.ctx, my_var.dst_index());
                        avcodec_parameters_from_context((*out_stream).codecpar, ctx);
                        self.stream_init.insert(*var);
                    }
                }
            },
            _ => return Err(Error::msg(format!("Payload not supported: {:?}", pkg))),
        }

        // Muxer never returns anything
        Ok(vec![])
    }
}
