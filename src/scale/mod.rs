use std::ffi::CStr;
use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, av_get_pix_fmt_name, AVFrame, SWS_BILINEAR,
    sws_freeContext, sws_getContext, sws_scale_frame, SwsContext,
};
use log::info;

use crate::pipeline::{AVFrameSource, PipelinePayload, PipelineProcessor};
use crate::return_ffmpeg_error;
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::StreamMapping;
use crate::variant::video::VideoVariant;

pub struct Scaler {
    variant: VideoVariant,
    ctx: *mut SwsContext,
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
    pub fn new(variant: VideoVariant) -> Self {
        Self {
            variant,
            ctx: ptr::null_mut(),
        }
    }

    unsafe fn setup_scaler(&mut self, frame: *const AVFrame) -> Result<(), Error> {
        if !self.ctx.is_null() {
            return Ok(());
        }

        let ctx = sws_getContext(
            (*frame).width,
            (*frame).height,
            transmute((*frame).format),
            self.variant.width as libc::c_int,
            self.variant.height as libc::c_int,
            transmute(self.variant.pixel_format),
            SWS_BILINEAR,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if ctx.is_null() {
            return Err(Error::msg("Failed to create scalar context"));
        }
        info!(
            "Scalar config: {}x{}@{} => {}x{}@{}",
            (*frame).width,
            (*frame).height,
            CStr::from_ptr(av_get_pix_fmt_name(transmute((*frame).format)))
                .to_str()?,
            self.variant.width,
            self.variant.height,
            CStr::from_ptr(av_get_pix_fmt_name(transmute(self.variant.pixel_format)))
                .to_str()?
        );
        self.ctx = ctx;
        Ok(())
    }

    unsafe fn process_frame(
        &mut self,
        frame: *mut AVFrame,
        src: &AVFrameSource,
    ) -> Result<Vec<PipelinePayload>, Error> {
        self.setup_scaler(frame)?;

        let dst_frame = av_frame_alloc();
        let ret = av_frame_copy_props(dst_frame, frame);
        return_ffmpeg_error!(ret);

        let ret = sws_scale_frame(self.ctx, dst_frame, frame);
        return_ffmpeg_error!(ret);

        Ok(vec![PipelinePayload::AvFrame(dst_frame, src.clone())])
    }
}

impl PipelineProcessor for Scaler {
    fn process(&mut self, pkg: PipelinePayload) -> Result<Vec<PipelinePayload>, Error> {
        match pkg {
            PipelinePayload::AvFrame(frm, ref src) => unsafe {
                let idx = match src {
                    AVFrameSource::Decoder(s) => (**s).index as usize,
                    AVFrameSource::None(s) => *s,
                    _ => return Err(Error::msg(format!("Cannot process frame from: {:?}", src))),
                };
                if self.variant.src_index() == idx {
                    self.process_frame(frm, src)
                } else {
                    Ok(vec![])
                }
            },
            PipelinePayload::Flush => {
                // pass flush to next step
                Ok(vec![pkg])
            }
            _ => Err(Error::msg("Payload not supported payload")),
        }
    }
}
