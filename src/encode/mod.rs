use ffmpeg_sys_next::{av_packet_rescale_ts, AVCodecContext, AVFrame, AVPacket, AVStream};
use ffmpeg_sys_next::AVMediaType::AVMEDIA_TYPE_VIDEO;

pub mod audio;
pub mod video;

/// Set packet details based on decoded frame
pub unsafe fn set_encoded_pkt_timing(
    ctx: *mut AVCodecContext,
    pkt: *mut AVPacket,
    in_frame: *mut AVFrame,
) {
    assert!(!(*in_frame).opaque.is_null());
    let in_stream = (*in_frame).opaque as *mut AVStream;
    let tb = (*ctx).time_base;
    (*pkt).stream_index = (*in_stream).index;
    if (*ctx).codec_type == AVMEDIA_TYPE_VIDEO {
        (*pkt).duration = tb.den as i64 / tb.num as i64 / (*in_stream).avg_frame_rate.num as i64
            * (*in_stream).avg_frame_rate.den as i64;
    }
    av_packet_rescale_ts(pkt, (*in_stream).time_base, (*ctx).time_base);
}
