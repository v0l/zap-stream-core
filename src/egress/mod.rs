use std::fmt::{Display, Formatter};
use std::ptr;

use anyhow::Error;
use ffmpeg_sys_next::{av_dump_format, avformat_new_stream, AVFormatContext, AVPacket};
use log::info;
use serde::{Deserialize, Serialize};

use crate::utils::id_ref_to_uuid;
use crate::variant::{VariantStream, VariantStreamType};

pub mod hls;
pub mod http;
pub mod mpegts;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EgressConfig {
    pub name: String,
    pub out_dir: String,
    pub variants: Vec<VariantStream>,
}

impl Display for EgressConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: out_dir={}", self.name, self.out_dir)?;
        if !self.variants.is_empty() {
            write!(f, "\n\tStreams: ")?;
            for v in &self.variants {
                write!(f, "\n\t\t{}", v)?;
            }
        }
        Ok(())
    }
}

pub unsafe fn map_variants_to_streams(
    ctx: *mut AVFormatContext,
    variants: &mut Vec<VariantStream>,
) -> Result<(), Error> {
    for var in variants {
        match var {
            VariantStream::Video(vs) => {
                let stream = avformat_new_stream(ctx, ptr::null());
                if stream.is_null() {
                    return Err(Error::msg("Failed to add stream to output"));
                }

                // overwrite dst_index to match output stream
                vs.dst_index = (*stream).index as usize;
                vs.to_stream(stream);
            }
            VariantStream::Audio(va) => {
                let stream = avformat_new_stream(ctx, ptr::null());
                if stream.is_null() {
                    return Err(Error::msg("Failed to add stream to output"));
                }

                // overwrite dst_index to match output stream
                va.dst_index = (*stream).index as usize;
                va.to_stream(stream);
            }
        }
    }

    av_dump_format(ctx, 0, ptr::null(), 1);
    Ok(())
}

pub unsafe fn get_pkt_variant(
    vars: &Vec<VariantStream>,
    pkt: *mut AVPacket,
) -> Result<&VariantStream, Error> {
    let variant_id = id_ref_to_uuid((*pkt).opaque_ref)?;
    let variant = vars.iter().find(|v| v.id() == variant_id);
    if variant.is_none() {
        return Err(Error::msg(format!(
            "No stream found with id={:?}",
            variant_id
        )));
    }
    Ok(variant.unwrap())
}

pub unsafe fn update_pkt_for_muxer(
    ctx: *mut AVFormatContext,
    pkt: *mut AVPacket,
    var: &VariantStream,
) {
    let stream = *(*ctx).streams.add(var.dst_index());
    let idx = (*stream).index;
    if idx != (*pkt).stream_index {
        (*pkt).stream_index = idx;
    }
}
