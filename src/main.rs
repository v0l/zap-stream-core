use std::ffi::CStr;

use clap::Parser;
use config::Config;
use log::{error, info};
use url::Url;

use crate::egress::http::listen_out_dir;
use crate::pipeline::builder::PipelineBuilder;
use crate::settings::Settings;
use crate::webhook::Webhook;

mod decode;
mod demux;
mod egress;
mod encode;
mod fraction;
mod ingress;
mod ipc;
mod pipeline;
mod scale;
mod settings;
mod utils;
mod variant;
mod webhook;

#[derive(Parser, Debug)]
struct Args {
    /// Add file input at startup
    #[arg(long)]
    file: Option<String>,

    /// Add input test pattern at startup
    #[arg(long)]
    test_pattern: bool,
}

/// Test:  ffmpeg -re -f lavfi -i testsrc -g 2 -r 30 -pix_fmt yuv420p -s 1280x720 -c:v h264 -b:v 2000k -f mpegts srt://localhost:3333
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let args = Args::parse();

    unsafe {
        //ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_DEBUG);
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

    let webhook = Webhook::new(settings.clone());
    let builder = PipelineBuilder::new(webhook);
    let mut listeners = vec![];
    for e in &settings.endpoints {
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
    listeners.push(tokio::spawn(listen_out_dir(
        "0.0.0.0:8080".to_owned(),
        settings.clone(),
    )));

    if let Some(p) = args.file {
        listeners.push(tokio::spawn(ingress::file::listen(
            p.parse()?,
            builder.clone(),
        )));
    }
    if args.test_pattern {
        listeners.push(tokio::spawn(ingress::test::listen(builder.clone())));
    }

    for handle in listeners {
        if let Err(e) = handle.await {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}

#[macro_export]
macro_rules! return_ffmpeg_error {
    ($x:expr) => {
        if $x < 0 {
                return Err(Error::msg(get_ffmpeg_error_msg($x)));
            }
    };
}
