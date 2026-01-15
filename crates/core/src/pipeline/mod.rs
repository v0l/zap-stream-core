use crate::listen::ListenerEndpoint;
use crate::overseer::Overseer;
use anyhow::bail;
use std::str::FromStr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod runner;

pub use runner::*;

pub(crate) mod worker;

mod config;
pub use config::*;

pub fn try_create_listener(
    u: &str,
    out_dir: &str,
    overseer: &Arc<dyn Overseer>,
    shutdown: CancellationToken,
) -> anyhow::Result<JoinHandle<anyhow::Result<()>>> {
    let ep = ListenerEndpoint::from_str(u)?;
    match ep {
        #[cfg(feature = "ingress-srt")]
        ListenerEndpoint::SRT { endpoint } => Ok(tokio::spawn(crate::ingress::srt::listen(
            out_dir.to_string(),
            endpoint,
            overseer.clone(),
            shutdown,
        ))),
        #[cfg(feature = "ingress-rtmp")]
        ListenerEndpoint::RTMP { endpoint } => Ok(tokio::spawn(crate::ingress::rtmp::listen(
            out_dir.to_string(),
            endpoint,
            overseer.clone(),
            shutdown,
        ))),
        #[cfg(feature = "ingress-tcp")]
        ListenerEndpoint::TCP { endpoint } => Ok(tokio::spawn(crate::ingress::tcp::listen(
            out_dir.to_string(),
            endpoint,
            overseer.clone(),
            shutdown,
        ))),
        ListenerEndpoint::File { path } => Ok(tokio::spawn(crate::ingress::file::listen(
            out_dir.to_string(),
            path,
            overseer.clone(),
        ))),
        #[cfg(feature = "ingress-test")]
        ListenerEndpoint::TestPattern => Ok(tokio::spawn(crate::ingress::test::listen(
            out_dir.to_string(),
            overseer.clone(),
            shutdown,
        ))),
        _ => {
            bail!("Unknown endpoint config: {u}");
        }
    }
}
