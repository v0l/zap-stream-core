mod decode;
mod demux;
mod egress;
mod encode;
mod fraction;
mod ingress;
mod pipeline;
mod scale;
mod settings;
mod utils;
mod variant;
mod webhook;
mod ipc;

use crate::pipeline::builder::PipelineBuilder;
use crate::settings::Settings;
use crate::webhook::Webhook;
use config::Config;
use futures_util::StreamExt;
use log::{error, info};
use std::ffi::CStr;
use futures_util::future::join_all;
use tokio::sync::futures;
use url::Url;

/// Test:  ffmpeg -re -f lavfi -i testsrc -g 2 -r 30 -pix_fmt yuv420p -s 1280x720 -c:v h264 -b:v 2000k -f mpegts srt://localhost:3333
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    unsafe {
        //ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_MAX_OFFSET);
        info!(
            "FFMPEG version={}",
            CStr::from_ptr(ffmpeg_sys_next::av_version_info())
                .to_str()
                .unwrap()
        );
    }

    let builder = Config::builder()
        .add_source(config::File::with_name("config.toml"))
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    let settings: Settings = builder.try_deserialize()?;

    let webhook = Webhook::new(settings.webhook_url);
    let builder = PipelineBuilder::new(webhook);
    let mut listeners = vec![];
    for e in settings.endpoints {
        let u: Url = e.parse()?;
        let addr = format!("{}:{}", u.host_str().unwrap(), u.port().unwrap());
        match u.scheme() {
            "srt" => listeners.push(tokio::spawn(ingress::srt::listen(addr, builder.clone()))),
            "tcp" => listeners.push(tokio::spawn(ingress::tcp::listen(addr, builder.clone()))),
            _ => {
                error!("Unknown endpoint config: {e}");
            }
        }
    }
    for handle in listeners {
        if let Err(e) = handle.await {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}
