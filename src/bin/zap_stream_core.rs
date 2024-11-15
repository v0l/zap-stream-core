use clap::Parser;
use config::Config;
use ffmpeg_rs_raw::ffmpeg_sys_the_third::av_version_info;
use ffmpeg_rs_raw::rstr;
use log::{error, info};
use url::Url;

use zap_stream_core::egress::http::listen_out_dir;
#[cfg(feature = "srt")]
use zap_stream_core::ingress::srt;
#[cfg(feature = "test-pattern")]
use zap_stream_core::ingress::test;

use zap_stream_core::ingress::{file, tcp};
use zap_stream_core::settings::Settings;

#[derive(Parser, Debug)]
struct Args {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let _args = Args::parse();

    unsafe {
        //ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_DEBUG);
        info!("FFMPEG version={}", rstr!(av_version_info()));
    }

    let builder = Config::builder()
        .add_source(config::File::with_name("config.yaml"))
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    let settings: Settings = builder.try_deserialize()?;
    let overseer = settings.get_overseer().await?;

    let mut listeners = vec![];
    for e in &settings.endpoints {
        let u: Url = e.parse()?;
        match u.scheme() {
            #[cfg(feature = "srt")]
            "srt" => listeners.push(tokio::spawn(srt::listen(
                u.host().unwrap().to_string(),
                overseer.clone(),
            ))),
            "tcp" => listeners.push(tokio::spawn(tcp::listen(
                u.host().unwrap().to_string(),
                overseer.clone(),
            ))),
            "file" => listeners.push(tokio::spawn(file::listen(
                u.path().parse()?,
                overseer.clone(),
            ))),
            #[cfg(feature = "test-pattern")]
            "test-pattern" => listeners.push(tokio::spawn(test::listen(overseer.clone()))),
            _ => {
                error!("Unknown endpoint config: {e}");
            }
        }
    }
    listeners.push(tokio::spawn(listen_out_dir(
        "0.0.0.0:8080".to_owned(),
        settings.output_dir,
    )));

    for handle in listeners {
        if let Err(e) = handle.await {
            error!("{e}");
        }
    }
    info!("Server closed");
    Ok(())
}
