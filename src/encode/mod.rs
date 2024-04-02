use std::ptr;

use ffmpeg_sys_next::{
    AV_LOG_INFO, AV_NOPTS_VALUE, av_packet_rescale_ts, av_pkt_dump_log2, AV_PKT_FLAG_KEY, av_q2d, AVCodecContext,
    AVPacket, AVRational, AVStream,
};
use ffmpeg_sys_next::AVMediaType::AVMEDIA_TYPE_VIDEO;
use log::info;

use crate::variant::VariantStreamType;

pub mod audio;
pub mod video;

/// Set packet details based on decoded frame
pub unsafe fn set_encoded_pkt_timing<TVar>(
    ctx: *mut AVCodecContext,
    pkt: *mut AVPacket,
    pts: &mut i64,
    var: &TVar,
) where
    TVar: VariantStreamType,
{
    let tb = (*ctx).time_base;
    (*pkt).stream_index = var.dst_index() as libc::c_int;
    (*pkt).time_base = var.time_base();
    if (*ctx).codec_type == AVMEDIA_TYPE_VIDEO && (*pkt).duration == 0 {
        let tb_sec = tb.den as i64 / tb.num as i64;
        let fps = (*ctx).framerate.num as i64 * (*ctx).framerate.den as i64;
        (*pkt).duration = tb_sec / fps;
    }
    if (*pkt).pts == AV_NOPTS_VALUE {
        (*pkt).pts = *pts;
        *pts += (*pkt).duration;
    } else {
        *pts = (*pkt).pts;
    }
    if (*pkt).dts == AV_NOPTS_VALUE {
        (*pkt).dts = (*pkt).pts;
    }
}

pub unsafe fn dump_pkt_info(pkt: *const AVPacket) {
    let tb = (*pkt).time_base;
    info!(
        "stream #{}: keyframe={}, duration={:.3}, dts={}, pts={}, size={}",
        (*pkt).stream_index,
        ((*pkt).flags & AV_PKT_FLAG_KEY) != 0,
        (*pkt).duration as f64 * av_q2d(tb),
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
        (*pkt).size
    );
}
