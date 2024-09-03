use std::ptr;
use std::time::Duration;

use anyhow::Error;
use bytes::{BufMut, Bytes};
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};
use ffmpeg_sys_next::*;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use crate::demux::info::{DemuxerInfo, StreamChannelType, StreamInfoChannel};
use crate::pipeline::{AVPacketSource, PipelinePayload};
use crate::return_ffmpeg_error;
use crate::utils::get_ffmpeg_error_msg;

pub mod info;

pub(crate) struct Demuxer {
    ctx: *mut AVFormatContext,
    started: Instant,
    state: DemuxerBuffer,
    info: Option<DemuxerInfo>,
}

unsafe impl Send for Demuxer {}

unsafe impl Sync for Demuxer {}

struct DemuxerBuffer {
    pub chan_in: UnboundedReceiver<Bytes>,
    pub buffer: bytes::BytesMut,
}

unsafe extern "C" fn read_data(
    opaque: *mut libc::c_void,
    buffer: *mut libc::c_uchar,
    size: libc::c_int,
) -> libc::c_int {
    let state = opaque as *mut DemuxerBuffer;
    loop {
        match (*state).chan_in.try_recv() {
            Ok(data) => {
                if !data.is_empty() {
                    (*state).buffer.put(data);
                }
                if (*state).buffer.len() >= size as usize {
                    let buf_take = (*state).buffer.split_to(size as usize);
                    memcpy(
                        buffer as *mut libc::c_void,
                        buf_take.as_ptr() as *const libc::c_void,
                        buf_take.len() as libc::c_ulonglong,
                    );
                    return size;
                } else {
                    continue;
                }
            }
            Err(e) => match e {
                TryRecvError::Empty => {}
                TryRecvError::Disconnected => {
                    return AVERROR_EOF;
                }
            },
        }
    }
}

impl Demuxer {
    pub fn new(chan_in: UnboundedReceiver<Bytes>) -> Self {
        unsafe {
            let ps = avformat_alloc_context();
            (*ps).flags |= AVFMT_FLAG_CUSTOM_IO;

            Self {
                ctx: ps,
                state: DemuxerBuffer {
                    chan_in,
                    buffer: bytes::BytesMut::new(),
                },
                info: None,
                started: Instant::now(),
            }
        }
    }

    unsafe fn probe_input(&mut self) -> Result<DemuxerInfo, Error> {
        const BUFFER_SIZE: usize = 4096;
        let buf_ptr = ptr::from_mut(&mut self.state) as *mut libc::c_void;
        let pb = avio_alloc_context(
            av_mallocz(BUFFER_SIZE) as *mut libc::c_uchar,
            BUFFER_SIZE as libc::c_int,
            0,
            buf_ptr,
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
        return_ffmpeg_error!(ret);

        if avformat_find_stream_info(self.ctx, ptr::null_mut()) < 0 {
            return Err(Error::msg("Could not find stream info"));
        }
        av_dump_format(self.ctx, 0, ptr::null_mut(), 0);

        let mut channel_infos = vec![];
        let video_stream_index =
            av_find_best_stream(self.ctx, AVMEDIA_TYPE_VIDEO, -1, -1, ptr::null_mut(), 0) as usize;
        if video_stream_index != AVERROR_STREAM_NOT_FOUND as usize {
            let video_stream = *(*self.ctx).streams.add(video_stream_index);
            channel_infos.push(StreamInfoChannel {
                index: video_stream_index,
                channel_type: StreamChannelType::Video,
                width: (*(*video_stream).codecpar).width as usize,
                height: (*(*video_stream).codecpar).height as usize,
                fps: av_q2d((*video_stream).avg_frame_rate) as f32,
                format: (*(*video_stream).codecpar).format as usize
            });
        }

        let audio_stream_index =
            av_find_best_stream(self.ctx, AVMEDIA_TYPE_AUDIO, -1, -1, ptr::null_mut(), 0) as usize;
        if audio_stream_index != AVERROR_STREAM_NOT_FOUND as usize {
            let audio_stream = *(*self.ctx).streams.add(audio_stream_index);
            let _codec_copy = unsafe {
                let ptr = avcodec_parameters_alloc();
                avcodec_parameters_copy(ptr, (*audio_stream).codecpar);
                ptr
            };
            channel_infos.push(StreamInfoChannel {
                index: audio_stream_index,
                channel_type: StreamChannelType::Audio,
                width: (*(*audio_stream).codecpar).width as usize,
                height: (*(*audio_stream).codecpar).height as usize,
                fps: 0.0,
                format: (*(*audio_stream).codecpar).format as usize
            });
        }

        let info = DemuxerInfo {
            channels: channel_infos,
            ctx: self.ctx,
        };
        Ok(info)
    }

    pub unsafe fn get_packet(&mut self) -> Result<PipelinePayload, Error> {
        let pkt: *mut AVPacket = av_packet_alloc();
        let ret = av_read_frame(self.ctx, pkt);
        if ret == AVERROR_EOF {
            return Ok(PipelinePayload::Flush);
        }
        if ret < 0 {
            let msg = get_ffmpeg_error_msg(ret);
            return Err(Error::msg(msg));
        }
        let stream = *(*self.ctx).streams.add((*pkt).stream_index as usize);
        let pkg = PipelinePayload::AvPacket(pkt, AVPacketSource::Demuxer(stream));
        Ok(pkg)
    }

    /// Try probe input stream
    pub fn try_probe(&mut self) -> Result<Option<DemuxerInfo>, Error> {
        match &self.info {
            None => {
                if (Instant::now() - self.started) > Duration::from_millis(500) {
                    let inf = unsafe { self.probe_input()? };
                    self.info = Some(inf.clone());
                    Ok(Some(inf))
                } else {
                    Ok(None)
                }
            }
            Some(i) => Ok(Some(i.clone())),
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
