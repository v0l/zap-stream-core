use ffmpeg_rs_raw::ffmpeg_sys_the_third::{AVFrame, av_frame_clone, av_frame_free};
use std::ops::Deref;

/// Safe wrapper around AVFrame
pub struct AvFrameRef {
    frame: *mut AVFrame,
}

impl Clone for AvFrameRef {
    fn clone(&self) -> Self {
        let clone = unsafe { av_frame_clone(self.frame) };
        Self { frame: clone }
    }
}

impl Drop for AvFrameRef {
    fn drop(&mut self) {
        unsafe {
            av_frame_free(&mut self.frame);
        }
        self.frame = std::ptr::null_mut();
    }
}

impl Deref for AvFrameRef {
    type Target = AVFrame;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.frame }
    }
}

impl AvFrameRef {
    /// Create a new AvFrameRef from a raw AVFrame pointer.
    /// Takes ownership of the frame - caller must not free it.
    pub unsafe fn new(frame: *mut AVFrame) -> Self {
        Self { frame }
    }

    pub fn ptr(&self) -> *mut AVFrame {
        self.frame
    }
}
