use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, AVFrame, SWS_BILINEAR,
    sws_freeContext, sws_getContext, sws_scale_frame, SwsContext,
};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;

use crate::pipeline::{AVFrameSource, PipelinePayload, PipelineProcessor};
use crate::utils::{get_ffmpeg_error_msg};
use crate::variant::VideoVariant;

pub struct Scaler {
    variant: VideoVariant,
    ctx: *mut SwsContext,
    chan_in: broadcast::Receiver<PipelinePayload>,
    chan_out: UnboundedSender<PipelinePayload>,
}

unsafe impl Send for Scaler {}

unsafe impl Sync for Scaler {}

impl Drop for Scaler {
    fn drop(&mut self) {
        unsafe {
            sws_freeContext(self.ctx);
            self.ctx = ptr::null_mut();
        }
    }
}

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

    unsafe fn process_frame(
        &mut self,
        frame: *mut AVFrame,
        src: &AVFrameSource,
    ) -> Result<(), Error> {
        let dst_fmt = transmute((*frame).format);

        if self.ctx.is_null() {
            let ctx = sws_getContext(
                (*frame).width,
                (*frame).height,
                transmute((*frame).format),
                self.variant.width as libc::c_int,
                self.variant.height as libc::c_int,
                dst_fmt,
                SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if ctx.is_null() {
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

        self.chan_out
            .send(PipelinePayload::AvFrame(dst_frame, src.clone()))?;
        Ok(())
    }
}

impl PipelineProcessor for Scaler {
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv() {
            match pkg {
                PipelinePayload::AvFrame(frm, ref src) => unsafe {
                    let idx = match src {
                        AVFrameSource::Decoder(s) => (**s).index,
                        _ => {
                            return Err(Error::msg(format!("Cannot process frame from: {:?}", src)))
                        }
                    };
                    if self.variant.src_index == idx as usize {
                        self.process_frame(frm, src)?;
                    }
                },
                _ => return Err(Error::msg("Payload not supported payload")),
            }
        }
        Ok(())
    }
}
