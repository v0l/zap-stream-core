use std::fmt::{Display, Formatter};
use std::ptr;

use crate::variant::{StreamMapping, VariantStream};
use anyhow::Error;
use ffmpeg_sys_next::{avformat_new_stream, AVFormatContext};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod hls;
pub mod http;
pub mod recorder;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EgressConfig {
    pub name: String,
    pub out_dir: String,
    /// Which variants will be used in this muxer
    pub variants: Vec<Uuid>,
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
    variants: &Vec<VariantStream>,
) -> Result<(), Error> {
    for var in variants {
        let stream = avformat_new_stream(ctx, ptr::null());
        if stream.is_null() {
            return Err(Error::msg("Failed to add stream to output"));
        }

        // replace stream index value with variant dst_index
        (*stream).index = var.dst_index() as libc::c_int;

        var.to_stream(stream);
    }
    Ok(())
}
