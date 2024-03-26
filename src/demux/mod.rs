use std::ptr;
use std::time::Duration;

use anyhow::Error;
use bytes::Bytes;
use ffmpeg_sys_next::*;
use ffmpeg_sys_next::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use crate::demux::info::{DemuxStreamInfo, StreamChannelType, StreamInfoChannel};
use crate::pipeline::PipelinePayload;
use crate::utils::get_ffmpeg_error_msg;

pub mod info;

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
    ctx: *mut AVFormatContext,
    chan_in: UnboundedReceiver<Bytes>,
    chan_out: UnboundedSender<PipelinePayload>,
    started: Instant,
}

unsafe impl Send for Demuxer {}

unsafe impl Sync for Demuxer {}

unsafe extern "C" fn read_data(
    opaque: *mut libc::c_void,
    buffer: *mut libc::c_uchar,
    size: libc::c_int,
) -> libc::c_int {
    let chan = opaque as *mut UnboundedReceiver<Bytes>;
    if let Some(mut data) = (*chan).blocking_recv() {
        let buff_len = data.len();
        let mut len = size.min(buff_len as libc::c_int);

        if len > 0 {
            memcpy(
                buffer as *mut libc::c_void,
                data.as_ptr() as *const libc::c_void,
                len as libc::c_ulonglong,
            );
        }
        len
    } else {
        AVERROR_EOF
    }
}

impl Demuxer {
    pub fn new(
        chan_in: UnboundedReceiver<Bytes>,
        chan_out: UnboundedSender<PipelinePayload>,
    ) -> Self {
        unsafe {
            let ps = avformat_alloc_context();
            (*ps).flags |= AVFMT_FLAG_CUSTOM_IO;

            Self {
                ctx: ps,
                chan_in,
                chan_out,
                started: Instant::now(),
            }
        }
    }

    unsafe fn probe_input(&mut self) -> Result<DemuxStreamInfo, Error> {
        let buf_ptr = ptr::from_mut(&mut self.chan_in) as *mut libc::c_void;
        let pb = avio_alloc_context(
            av_mallocz(4096) as *mut libc::c_uchar,
            4096,
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
        if ret < 0 {
            let msg = get_ffmpeg_error_msg(ret);
            return Err(Error::msg(msg));
        }
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
            });
        }

        let audio_stream_index =
            av_find_best_stream(self.ctx, AVMEDIA_TYPE_AUDIO, -1, -1, ptr::null_mut(), 0) as usize;
        if audio_stream_index != AVERROR_STREAM_NOT_FOUND as usize {
            let audio_stream = *(*self.ctx).streams.add(audio_stream_index);
            let codec_copy = unsafe {
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
            });
        }

        let info = DemuxStreamInfo {
            channels: channel_infos,
        };
        Ok(info)
    }

    unsafe fn get_packet(&mut self) -> Result<(), Error> {
        let pkt: *mut AVPacket = av_packet_alloc();
        let ret = av_read_frame(self.ctx, pkt);
        if ret == AVERROR_EOF {
            return Err(Error::msg("Stream EOF"));
        }
        if ret < 0 {
            let msg = get_ffmpeg_error_msg(ret);
            return Err(Error::msg(msg));
        }
        let stream = *(*self.ctx).streams.add((*pkt).stream_index as usize);
        if (*pkt).time_base.num == 0 {
            (*pkt).time_base = (*stream).time_base;
        }
        (*pkt).opaque = stream as *mut libc::c_void;

        let pkg = PipelinePayload::AvPacket("Demuxer packet".to_owned(), pkt);
        self.chan_out.send(pkg)?;
        Ok(())
    }

    pub fn process(&mut self) -> Result<Option<DemuxStreamInfo>, Error> {
        unsafe {
            let score = (*self.ctx).probe_score;
            if score < 30 {
                if (Instant::now() - self.started) > Duration::from_secs(1) {
                    return Ok(Some(self.probe_input()?));
                }
                return Ok(None);
            }
            self.get_packet()?;
            Ok(None)
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
