use crate::overseer::Overseer;
use anyhow::bail;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone)]
pub enum ListenerEndpoint {
    SRT { endpoint: String },
    RTMP { endpoint: String },
    TCP { endpoint: String },
    File { path: PathBuf },
    TestPattern,
}

impl FromStr for ListenerEndpoint {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url: Url = s.parse()?;
        match url.scheme() {
            "srt" => Ok(Self::SRT {
                endpoint: format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            }),
            "rtmp" => Ok(Self::RTMP {
                endpoint: format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            }),
            "tcp" => Ok(Self::TCP {
                endpoint: format!("{}:{}", url.host().unwrap(), url.port().unwrap()),
            }),
            "file" => Ok(Self::File {
                path: PathBuf::from(url.path()),
            }),
            "test-pattern" => Ok(Self::TestPattern),
            _ => bail!("Unsupported endpoint scheme: {}", url.scheme()),
        }
    }
}

impl ListenerEndpoint {
    pub fn to_public_url(&self, public_hostname: &str, ingest_name: &str) -> Option<String> {
        match self {
            ListenerEndpoint::SRT { endpoint } => {
                if let Ok(addr) = endpoint.parse::<Url>() {
                    Some(format!(
                        "srt://{}:{}",
                        public_hostname,
                        if let Some(p) = addr.port() {
                            format!(":{}", p)
                        } else {
                            return None;
                        }
                    ))
                } else {
                    None
                }
            }
            ListenerEndpoint::RTMP { endpoint } => {
                if let Ok(addr) = endpoint.parse::<Url>() {
                    Some(format!(
                        "rtmp://{}{}/{}",
                        public_hostname,
                        if let Some(p) = addr.port() {
                            format!(":{}", p)
                        } else {
                            "".to_string()
                        },
                        ingest_name
                    ))
                } else {
                    None
                }
            }
            ListenerEndpoint::TCP { endpoint } => {
                if let Ok(addr) = endpoint.parse::<Url>() {
                    Some(format!(
                        "tcp://{}:{}",
                        public_hostname,
                        if let Some(p) = addr.port() {
                            format!(":{}", p)
                        } else {
                            return None;
                        }
                    ))
                } else {
                    None
                }
            }
            ListenerEndpoint::File { .. } => None,
            ListenerEndpoint::TestPattern => None,
        }
    }
}

/// Try to span a listener
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
