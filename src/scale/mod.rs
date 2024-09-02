use std::ffi::CStr;
use std::mem::transmute;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{
    av_frame_alloc, av_frame_copy_props, av_get_pix_fmt_name, AVFrame,
    SWS_BILINEAR, sws_freeContext, sws_getContext, sws_scale_frame, SwsContext,
};
use log::info;
use tokio::sync::mpsc::UnboundedSender;

use crate::ipc::Rx;
use crate::pipeline::{AVFrameSource, PipelinePayload, PipelineProcessor};
use crate::utils::get_ffmpeg_error_msg;
use crate::variant::VideoVariant;

pub struct Scaler<T> {
    variant: VideoVariant,
    ctx: *mut SwsContext,
    chan_in: T,
    chan_out: UnboundedSender<PipelinePayload>,
}

unsafe impl<TRecv> Send for Scaler<TRecv> {}

unsafe impl<TRecv> Sync for Scaler<TRecv> {}

impl<TRecv> Drop for Scaler<TRecv> {
    fn drop(&mut self) {
        unsafe {
            sws_freeContext(self.ctx);
            self.ctx = ptr::null_mut();
        }
    }
}

impl<TRecv> Scaler<TRecv>
where
    TRecv: Rx<PipelinePayload>,
{
    pub fn new(
        chan_in: TRecv,
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
        if self.ctx.is_null() {
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
                    .to_str()
                    .unwrap(),
                self.variant.width,
                self.variant.height,
                CStr::from_ptr(av_get_pix_fmt_name(transmute(self.variant.pixel_format)))
                    .to_str()
                    .unwrap()
            );
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

impl<TRecv> PipelineProcessor for Scaler<TRecv>
where
    TRecv: Rx<PipelinePayload>,
{
    fn process(&mut self) -> Result<(), Error> {
        while let Ok(pkg) = self.chan_in.try_recv_next() {
            match pkg {
                PipelinePayload::AvFrame(frm, ref src) => unsafe {
                    let idx = match src {
                        AVFrameSource::Decoder(s) => (**s).index as usize,
                        AVFrameSource::None(s) => *s,
                        _ => {
                            return Err(Error::msg(format!("Cannot process frame from: {:?}", src)))
                        }
                    };
                    if self.variant.src_index == idx {
                        self.process_frame(frm, src)?;
                    }
                },
                PipelinePayload::Flush => {
                    // pass flush to next step
                    self.chan_out.send(PipelinePayload::Flush)?;
                }
                _ => return Err(Error::msg("Payload not supported payload")),
            }
        }
        Ok(())
    }
}
