use std::ffi::CStr;
use std::pin::Pin;
use std::ptr;

use anyhow::Error;
use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use ffmpeg_sys_next::*;
use log::{debug, info, warn};

use crate::pipeline::{PipelinePayload, PipelineStep};

///
/// Demuxer supports demuxing and decoding
///
/// | Type   | Value                         |
/// | ------ | ----------------------------- |
/// | Video  | H264, H265, VP8, VP9, AV1     |
/// | Audio  | AAC, Opus                     |
/// | Format | MPEG-TS                       |
///
pub(crate) struct Demuxer {
    buffer: BytesMut,
    ctx: *mut AVFormatContext,
}

unsafe impl Send for Demuxer {}
unsafe impl Sync for Demuxer {}

unsafe extern "C" fn read_data(
    opaque: *mut libc::c_void,
    buffer: *mut libc::c_uchar,
    size: libc::c_int,
) -> libc::c_int {
    let muxer = opaque as *mut Demuxer;
    let len = size.min((*muxer).buffer.len() as libc::c_int);
    if len > 0 {
        memcpy(
            buffer as *mut libc::c_void,
            (*muxer).buffer.as_ptr() as *const libc::c_void,
            len as libc::c_ulonglong,
        );
        _ = (*muxer).buffer.split_to(len as usize);
        len
    } else {
        AVERROR_BUFFER_TOO_SMALL
    }
}

impl Demuxer {
    const BUFFER_SIZE: usize = 1024 * 1024;
    const INIT_BUFFER_THRESHOLD: usize = 2048;

    pub fn new() -> Self {
        unsafe {
            let ps = avformat_alloc_context();
            (*ps).probesize = Self::BUFFER_SIZE as i64;
            (*ps).flags |= AVFMT_FLAG_CUSTOM_IO;
            Self {
                ctx: ps,
                buffer: BytesMut::with_capacity(Self::BUFFER_SIZE),
            }
        }
    }

    unsafe fn append_buffer(&mut self, bytes: &Bytes) {
        self.buffer.extend_from_slice(bytes);
    }

    unsafe fn probe_input(&mut self) -> Result<bool, Error> {
        let size = self.buffer.len();
        let score = (*self.ctx).probe_score;
        if score == 0 && size >= Self::INIT_BUFFER_THRESHOLD {
            let pb = avio_alloc_context(
                av_mallocz(4096) as *mut libc::c_uchar,
                4096,
                0,
                self as *const Self as *mut libc::c_void,
                Some(read_data),
                None,
                None,
            );

            (*self.ctx).pb = pb;
            let ret = avformat_open_input(
                &mut self.ctx,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if ret < 0 {
                let msg = Self::get_ffmpeg_error_msg(ret);
                return Err(Error::msg(msg));
            }

            if avformat_find_stream_info(self.ctx, ptr::null_mut()) < 0 {
                return Err(Error::msg("Could not find stream info"));
            }

            for x in 0..(*self.ctx).nb_streams {
                av_dump_format(self.ctx, x as libc::c_int, ptr::null_mut(), 0);
            }
        }
        Ok(score > 0)
    }

    unsafe fn decode_packet(&mut self) -> Result<Option<*mut AVPacket>, Error> {
        let pkt: *mut AVPacket = av_packet_alloc();
        av_init_packet(pkt);

        let ret = av_read_frame(self.ctx, pkt);
        if ret == AVERROR_BUFFER_TOO_SMALL {
            return Ok(None);
        }
        if ret < 0 {
            let msg = Self::get_ffmpeg_error_msg(ret);
            return Err(Error::msg(msg));
        }
        Ok(Some(pkt))
    }

    unsafe fn print_buffer_info(&mut self) {
        let mut pb = (*self.ctx).pb;
        let offset = (*pb).pos;
        let remaining = (*pb).buffer_size as i64 - (*pb).pos;
        info!("offset={}, remaining={}", offset, remaining);
    }

    fn get_ffmpeg_error_msg(ret: libc::c_int) -> String {
        unsafe {
            let mut buf: [libc::c_char; 255] = [0; 255];
            av_make_error_string(buf.as_mut_ptr(), 255, ret);
            String::from(CStr::from_ptr(buf.as_ptr()).to_str().unwrap())
        }
    }
}

impl Drop for Demuxer {
    fn drop(&mut self) {
        unsafe {
            avformat_free_context(self.ctx);
            self.ctx = ptr::null_mut();
        }
    }
}

#[async_trait]
impl PipelineStep for Demuxer {
    fn name(&self) -> String {
        "Demuxer".to_owned()
    }

    async fn process(&mut self, pkg: PipelinePayload) -> Result<PipelinePayload, Error> {
        match pkg {
            PipelinePayload::Bytes(ref bb) => unsafe {
                self.append_buffer(bb);
                if !self.probe_input()? {
                    return Ok(PipelinePayload::Empty);
                }
                match self.decode_packet() {
                    Ok(pkt) => match pkt {
                        Some(pkt) => Ok(PipelinePayload::AvPacket(pkt)),
                        None => Ok(PipelinePayload::Empty),
                    },
                    Err(e) => {
                        warn!("{}", e);
                        Ok(PipelinePayload::Empty)
                    }
                }
            },
            _ => return Err(Error::msg("Wrong pkg format")),
        }
    }
}
