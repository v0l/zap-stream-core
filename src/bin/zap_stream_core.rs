use clap::Parser;
use config::Config;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::av_version_info;
use ffmpeg_rs_raw::rstr;
use log::{error, info};
use url::Url;

use zap_stream_core::egress::http::listen_out_dir;
#[cfg(feature = "srt")]
use zap_stream_core::ingress::srt;
use zap_stream_core::ingress::{file, tcp, test};
use zap_stream_core::settings::Settings;


#[derive(Parser, Debug)]
struct Args {
    /// Add file input at startup
    #[arg(long)]
    file: Option<String>,

    /// Add input test pattern at startup
    #[cfg(feature = "test-source")]
    #[arg(long)]
    test_pattern: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let args = Args::parse();

    unsafe {
        //ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_DEBUG);
        info!("FFMPEG version={}", rstr!(av_version_info()));
    }

    let builder = Config::builder()
        .add_source(config::File::with_name("config.toml"))
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    let settings: Settings = builder.try_deserialize()?;

    let mut listeners = vec![];
    for e in &settings.endpoints {
        let u: Url = e.parse()?;
        let addr = format!("{}:{}", u.host_str().unwrap(), u.port().unwrap());
        match u.scheme() {
            #[cfg(feature = "srt")]
            "srt" => listeners.push(tokio::spawn(srt::listen(addr, settings.clone()))),
            "tcp" => listeners.push(tokio::spawn(tcp::listen(addr, settings.clone()))),
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
        listeners.push(tokio::spawn(file::listen(
            p.parse()?,
            settings.clone(),
        )));
    }
    #[cfg(feature = "test-source")]
    if args.test_pattern {
        listeners.push(tokio::spawn(test::listen(settings.clone())));
    }

    for handle in listeners {
        if let Err(e) = handle.await {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}
