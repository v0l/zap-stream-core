use std::collections::HashMap;
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{av_frame_alloc, AVCodec, avcodec_alloc_context3, avcodec_find_decoder, avcodec_free_context, avcodec_open2, avcodec_parameters_copy, avcodec_parameters_to_context, avcodec_receive_frame, avcodec_send_packet, AVCodecContext, AVERROR, AVERROR_EOF, AVPacket};
use ffmpeg_sys_next::AVPictureType::AV_PICTURE_TYPE_NONE;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::encode::set_encoded_pkt_timing;
use crate::pipeline::{AVFrameSource, AVPacketSource, PipelinePayload};
use crate::variant::{VariantStream, VideoVariant};

struct CodecContext {
    pub context: *mut AVCodecContext,
    pub codec: *const AVCodec,
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        unsafe {
            avcodec_free_context(&mut self.context);
            self.codec = ptr::null_mut();
            self.context = ptr::null_mut();
        }
    }
}

pub struct Decoder {
    chan_in: UnboundedReceiver<PipelinePayload>,
    chan_out: broadcast::Sender<PipelinePayload>,
    codecs: HashMap<i32, CodecContext>,
    pts: i64,
}

unsafe impl Send for Decoder {}

unsafe impl Sync for Decoder {}

impl Decoder {
    pub fn new(
        chan_in: UnboundedReceiver<PipelinePayload>,
        chan_out: broadcast::Sender<PipelinePayload>,
    ) -> Self {
        Self {
            chan_in,
            chan_out,
            codecs: HashMap::new(),
            pts: 0,
        }
    }

    pub unsafe fn decode_pkt(
        &mut self,
        pkt: *mut AVPacket,
        src: &AVPacketSource,
    ) -> Result<usize, Error> {
        let stream_index = (*pkt).stream_index;
        let stream = match src {
            AVPacketSource::Demuxer(s) => *s,
            _ => return Err(Error::msg(format!("Cannot decode packet from: {:?}", src))),
        };

        assert_eq!(
            stream_index,
            (*stream).index,
            "Passed stream reference does not match stream_index of packet"
        );

        let codec_par = (*stream).codecpar;
        assert_ne!(
            codec_par,
            ptr::null_mut(),
            "Codec parameters are missing from stream"
        );

        if let std::collections::hash_map::Entry::Vacant(e) = self.codecs.entry(stream_index) {
            let codec = avcodec_find_decoder((*codec_par).codec_id);
            if codec.is_null() {
                return Err(Error::msg("Failed to find codec"));
            }
            let context = avcodec_alloc_context3(ptr::null());
            if context.is_null() {
                return Err(Error::msg("Failed to alloc context"));
            }
            if avcodec_parameters_to_context(context, (*stream).codecpar) != 0 {
                return Err(Error::msg("Failed to copy codec parameters to context"));
            }
            (*context).pkt_timebase = (*stream).time_base;
            if avcodec_open2(context, codec, ptr::null_mut()) < 0 {
                return Err(Error::msg("Failed to open codec"));
            }
            e.insert(CodecContext { context, codec });
        }
        if let Some(ctx) = self.codecs.get_mut(&stream_index) {
            let mut ret = avcodec_send_packet(ctx.context, pkt);
            if ret < 0 {
                return Err(Error::msg(format!("Failed to decode packet {}", ret)));
            }

            let mut frames = 0;
            while ret >= 0 {
                let frame = av_frame_alloc();
                ret = avcodec_receive_frame(ctx.context, frame);
                if ret < 0 {
                    if ret == AVERROR_EOF || ret == AVERROR(libc::EAGAIN) {
                        break;
                    }
                    return Err(Error::msg(format!("Failed to decode {}", ret)));
                }

                (*frame).pict_type = AV_PICTURE_TYPE_NONE; // encoder prints warnings
                self.chan_out.send(PipelinePayload::AvFrame(
                    frame,
                    AVFrameSource::Decoder(stream),
                ))?;
                frames += 1;
            }
            return Ok(frames);
        }
        Ok(0)
    }

    pub fn process(&mut self) -> Result<usize, Error> {
        if let Ok(pkg) = self.chan_in.try_recv() {
            return if let PipelinePayload::AvPacket(pkt, ref src) = pkg {
                unsafe {
                    let frames = self.decode_pkt(pkt, src)?;
                    Ok(frames)
                }
            } else {
                Err(Error::msg("Payload not supported"))
            };
        }
        Ok(0)
    }
}
