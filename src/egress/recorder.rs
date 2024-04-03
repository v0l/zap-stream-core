use std::{fs, ptr};
use std::collections::HashSet;
use std::fmt::Display;

use anyhow::Error;
use ffmpeg_sys_next::{av_dump_format, av_guess_format, av_interleaved_write_frame, av_strdup, avformat_alloc_context, avformat_alloc_output_context2, avformat_free_context, avformat_write_header, AVFormatContext, AVIO_FLAG_READ_WRITE, avio_flush, avio_open2, AVPacket};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::egress::{EgressConfig, get_pkt_variant, map_variants_to_streams, update_pkt_for_muxer};
use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::get_ffmpeg_error_msg;

pub struct RecorderEgress {
    id: Uuid,
    config: EgressConfig,
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<PipelinePayload>,
    stream_init: HashSet<i32>,
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

        let out_file = format!("{}/recording.mkv\0", base).as_ptr() as *const libc::c_char;
        fs::create_dir_all(base.clone())?;
        let ret = avio_open2(
            &mut (*ctx).pb,
            out_file,
            AVIO_FLAG_READ_WRITE,
            ptr::null(),
            ptr::null_mut(),
        );
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }
        (*ctx).oformat = av_guess_format(
            "matroska\0".as_ptr() as *const libc::c_char,
            out_file,
            ptr::null(),
        );
        if (*ctx).oformat.is_null() {
            return Err(Error::msg("Output format not found"));
        }
        (*ctx).url = av_strdup(out_file);
        map_variants_to_streams(ctx, &mut self.config.variants)?;

        let ret = avformat_write_header(ctx, ptr::null_mut());
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }
        av_dump_format(ctx, 0, ptr::null(), 1);
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

impl PipelineProcessor for RecorderEgress {
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