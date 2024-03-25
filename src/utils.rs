use std::ffi::CStr;

use anyhow::Error;
use ffmpeg_sys_next::{av_buffer_allocz, av_make_error_string, AVBufferRef, memcpy};
use uuid::{Bytes, Uuid};

use crate::variant::{VariantStream, VideoVariant};

pub fn get_ffmpeg_error_msg(ret: libc::c_int) -> String {
    unsafe {
        const BUF_SIZE: usize = 512;
        let mut buf: [libc::c_char; BUF_SIZE] = [0; BUF_SIZE];
        av_make_error_string(buf.as_mut_ptr(), BUF_SIZE, ret);
        String::from(CStr::from_ptr(buf.as_ptr()).to_str().unwrap())
    }
}

pub fn variant_id_ref(var: &VariantStream) -> Result<*mut AVBufferRef, Error> {
    unsafe {
        match var {
            VariantStream::Audio(va) => {
                let buf = av_buffer_allocz(16);
                memcpy(
                    (*buf).data as *mut libc::c_void,
                    va.id.as_bytes().as_ptr() as *const libc::c_void,
                    16,
                );
                Ok(buf)
            }
            VariantStream::Video(vv) => {
                let buf = av_buffer_allocz(16);
                memcpy(
                    (*buf).data as *mut libc::c_void,
                    vv.id.as_bytes().as_ptr() as *const libc::c_void,
                    16,
                );
                Ok(buf)
            }
            _ => return Err(Error::msg("Cannot assign pkt stream index")),
        }
    }
}

pub fn video_variant_id_ref(var: &VideoVariant) -> *mut AVBufferRef {
    unsafe {
        let buf = av_buffer_allocz(16);
        memcpy(
            (*buf).data as *mut libc::c_void,
            var.id.as_bytes().as_ptr() as *const libc::c_void,
            16,
        );
        buf
    }
}

pub fn id_ref_to_uuid(buf: *mut AVBufferRef) -> Uuid {
    unsafe {
        let binding = Bytes::from(*((*buf).data as *const [u8; 16]));
        Uuid::from_bytes_ref(&binding).clone()
    }
}
