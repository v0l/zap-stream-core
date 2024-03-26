use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_buffer_ref, av_frame_alloc, av_frame_copy_props, AVBufferRef, AVFrame,
    SWS_BILINEAR, sws_getContext, sws_scale_frame, SwsContext,
};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;

use crate::pipeline::{PipelinePayload, PipelineProcessor};
use crate::utils::{get_ffmpeg_error_msg, video_variant_id_ref};
use crate::variant::VideoVariant;

pub struct Scaler {
    variant: VideoVariant,
    ctx: *mut SwsContext,
    chan_in: broadcast::Receiver<PipelinePayload>,
    chan_out: UnboundedSender<PipelinePayload>,
    var_id_ref: *mut AVBufferRef,
}

unsafe impl Send for Scaler {}

unsafe impl Sync for Scaler {}

impl Scaler {
    pub fn new(
        chan_in: broadcast::Receiver<PipelinePayload>,
        chan_out: UnboundedSender<PipelinePayload>,
        variant: VideoVariant,
    ) -> Self {
        let id_ref = video_variant_id_ref(&variant);
        Self {
            chan_in,
            chan_out,
            variant,
            ctx: ptr::null_mut(),
            var_id_ref: id_ref,
        }
    }

    unsafe fn process_frame(&mut self, frame: *mut AVFrame, src_index: usize) -> Result<(), Error> {
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
        if ret < 0 {
            return Err(Error::msg(get_ffmpeg_error_msg(ret)));
        }

        (*dst_frame).opaque_ref = av_buffer_ref(self.var_id_ref);

        self.chan_out.send(PipelinePayload::AvFrame(
            "Scaler frame".to_owned(),
            dst_frame,
            src_index
        ))?;
        Ok(())
    }
}

impl PipelineProcessor for Scaler {
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            match pkg {
                PipelinePayload::AvFrame(_, frm, idx) => unsafe {
                    if self.variant.src_index == idx {
                        self.process_frame(frm, idx)?;
                    }
                },
                _ => return Err(Error::msg("Payload not supported payload")),
            }
        }
        Ok(())
    }
}
