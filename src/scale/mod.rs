use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, av_frame_unref, AVFrame, SWS_BILINEAR, sws_getContext,
    sws_scale_frame, SwsContext,
};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;

use crate::pipeline::PipelinePayload;
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::VideoVariant;

pub struct Scaler {
    variant: VideoVariant,
    ctx: *mut SwsContext,
    chan_in: broadcast::Receiver<PipelinePayload>,
    chan_out: UnboundedSender<PipelinePayload>,
}

unsafe impl Send for Scaler {}
unsafe impl Sync for Scaler {}

impl Scaler {
    pub fn new(
        chan_in: broadcast::Receiver<PipelinePayload>,
        chan_out: UnboundedSender<PipelinePayload>,
        variant: VideoVariant,
    ) -> Self {
        Self {
            chan_in,
            chan_out,
            variant,
            ctx: ptr::null_mut(),
        }
    }

    unsafe fn process_frame(&mut self, frame: *mut AVFrame) -> Result<(), Error> {
        if (*frame).width == 0 {
            // only picture frames supported
            return Ok(());
        }
        let dst_fmt = transmute((*frame).format);

        if self.ctx == ptr::null_mut() {
            let ctx = sws_getContext(
                (*frame).width,
                (*frame).height,
                dst_fmt,
                self.variant.width as libc::c_int,
                self.variant.height as libc::c_int,
                dst_fmt,
                SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if ctx == ptr::null_mut() {
                return Err(Error::msg("Failed to create scalar context"));
            }
            self.ctx = ctx;
        }

        let dst_frame = av_frame_alloc();
        let ret = av_frame_copy_props(dst_frame, frame);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        let ret = sws_scale_frame(self.ctx, dst_frame, frame);
        av_frame_unref(frame);
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        (*dst_frame).time_base = (*frame).time_base;
        (*dst_frame).pts = (*frame).pts;
        (*dst_frame).pkt_dts = (*frame).pkt_dts;
        self.chan_out.send(PipelinePayload::AvFrame(dst_frame))?;
        Ok(())
    }

    pub fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            match pkg {
                PipelinePayload::AvFrame(frm) => unsafe {
                    self.process_frame(frm)?;
                },
                _ => return Err(Error::msg("Payload not supported payload")),
            }
        }
        Ok(())
    }
}
