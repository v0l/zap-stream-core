use std::{fs, ptr};
use std::collections::HashSet;
use std::fmt::Display;

use anyhow::Error;
use ffmpeg_sys_next::{av_guess_format, av_interleaved_write_frame, av_strdup, avcodec_parameters_from_context, AVCodecContext, avformat_alloc_context, avformat_free_context, avformat_write_header, AVFormatContext, AVIO_FLAG_READ_WRITE, avio_open2, AVPacket};
use itertools::Itertools;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::egress::{EgressConfig, get_pkt_variant, map_variants_to_streams, update_pkt_for_muxer};
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::VariantStreamType;

pub struct MPEGTSEgress {
    id: Uuid,
    config: EgressConfig,
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<PipelinePayload>,
    stream_init: HashSet<i32>,
}

unsafe impl Send for MPEGTSEgress {}

unsafe impl Sync for MPEGTSEgress {}

impl Drop for MPEGTSEgress {
    fn drop(&mut self) {
        unsafe {
            avformat_free_context(self.ctx);
            self.ctx = ptr::null_mut();
        }
    }
}

impl MPEGTSEgress {
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
            stream_init: HashSet::new(),
        }
    }

    unsafe fn setup_muxer(&mut self) -> Result<(), Error> {
        let mut ctx = avformat_alloc_context();
        if ctx.is_null() {
            return Err(Error::msg("Failed to create muxer context"));
        }
        let base = format!("{}/{}", self.config.out_dir, self.id);

        fs::create_dir_all(base.clone())?;
        let ret = avio_open2(
            &mut (*ctx).pb,
            format!("{}/live.ts\0", base).as_ptr() as *const libc::c_char,
            AVIO_FLAG_READ_WRITE,
            ptr::null(),
            ptr::null_mut(),
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }
        (*ctx).oformat = av_guess_format(
            "mpegts\0".as_ptr() as *const libc::c_char,
            ptr::null(),
            ptr::null(),
        );
        if (*ctx).oformat.is_null() {
            return Err(Error::msg("Output format not found"));
        }
        (*ctx).url = av_strdup(format!("{}/live.ts\0", base).as_ptr() as *const libc::c_char);
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

        let ret = av_interleaved_write_frame(self.ctx, pkt);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        Ok(())
    }
}

impl PipelineProcessor for MPEGTSEgress {
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
