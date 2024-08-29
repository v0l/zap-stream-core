use std::ffi::CStr;

use anyhow::Error;
use ffmpeg_sys_next::{av_buffer_allocz, av_make_error_string, AVBufferRef, memcpy};
use uuid::{Bytes, Uuid};

use crate::variant::{AudioVariant, VariantStream, VideoVariant};

pub fn get_ffmpeg_error_msg(ret: libc::c_int) -> String {
    unsafe {
        const BUF_SIZE: usize = 512;
        let mut buf: [libc::c_char; BUF_SIZE] = [0; BUF_SIZE];
        av_make_error_string(buf.as_mut_ptr(), BUF_SIZE, ret);
        String::from(CStr::from_ptr(buf.as_ptr()).to_str().unwrap())
    }
}