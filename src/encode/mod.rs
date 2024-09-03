use ffmpeg_sys_next::AVMediaType::AVMEDIA_TYPE_VIDEO;
use ffmpeg_sys_next::{
    av_packet_rescale_ts, AVCodecContext, AVPacket, AVRational, AV_NOPTS_VALUE, AV_PKT_FLAG_KEY,
};
use log::info;

use crate::variant::{EncodedStream, StreamMapping};

pub mod audio;
pub mod video;

/// Set packet details based on decoded frame
pub unsafe fn set_encoded_pkt_timing<TVar>(
    ctx: *mut AVCodecContext,
    pkt: *mut AVPacket,
    in_tb: &AVRational,
    pts: &mut i64,
    var: &TVar,
) where
    TVar: EncodedStream + StreamMapping,
{
    let out_tb = (*ctx).time_base;

    (*pkt).stream_index = var.dst_index() as libc::c_int;
    if (*pkt).duration <= 0 && (*ctx).codec_type == AVMEDIA_TYPE_VIDEO {
        let tb_sec = in_tb.den as i64 / in_tb.num as i64;
        let fps = (*ctx).framerate.num as i64 * (*ctx).framerate.den as i64;
        (*pkt).duration = tb_sec / if fps == 0 { 1 } else { fps }
    }

    av_packet_rescale_ts(pkt, *in_tb, out_tb);
    (*pkt).time_base = var.time_base();
    (*pkt).pos = -1;
    if (*pkt).pts == AV_NOPTS_VALUE {
        (*pkt).pts = *pts;
        *pts += (*pkt).duration;
    }
    if (*pkt).dts == AV_NOPTS_VALUE {
        (*pkt).dts = (*pkt).pts;
    }
}

pub unsafe fn dump_pkt_info(pkt: *const AVPacket) {
    let tb = (*pkt).time_base;
    info!(
        "stream {}: keyframe={}, duration={}, dts={}, pts={}, size={}, tb={}/{}",
        (*pkt).stream_index,
        ((*pkt).flags & AV_PKT_FLAG_KEY) != 0,
        (*pkt).duration,
        if (*pkt).dts == AV_NOPTS_VALUE {
            "N/A".to_owned()
        } else {
            format!("{}", (*pkt).dts)
        },
        if (*pkt).pts == AV_NOPTS_VALUE {
            "N/A".to_owned()
        } else {
            format!("{}", (*pkt).pts)
        },
        (*pkt).size,
        tb.num,
        tb.den
    );
}
