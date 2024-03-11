mod pipeline;
mod ingress;
mod webhook;
mod demux;

use std::ffi::CStr;
use futures_util::StreamExt;
use log::info;
use crate::pipeline::builder::PipelineBuilder;
use crate::webhook::Webhook;

/// Test:  ffmpeg -re -f lavfi -i testsrc -g 2 -r 30 -pix_fmt yuv420p -s 1280x720 -c:v h264 -b:v 2000k -f mpegts srt://localhost:3333
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    unsafe {
        ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_INFO);
        info!("{}", CStr::from_ptr(ffmpeg_sys_next::av_version_info()).to_str().unwrap());
    }

    let webhook = Webhook::new("".to_owned());
    let builder = PipelineBuilder::new(webhook);
    let srt = tokio::spawn(ingress::srt::listen_srt(3333, builder));

    srt.await?.expect("TODO: panic message");

    println!("\nServer closed");
    Ok(())
}
