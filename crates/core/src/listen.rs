use anyhow::bail;
use anyhow::{Result, anyhow};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;

#[derive(Clone)]
pub enum ListenerEndpoint {
    SRT { endpoint: Url },
    RTMP { endpoint: Url },
    TCP { addr: SocketAddr },
    File { path: PathBuf },
    TestPattern,
}

impl FromStr for ListenerEndpoint {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url: Url = s.parse()?;
        match url.scheme() {
            "srt" => Ok(Self::SRT { endpoint: url }),
            "rtmp" => Ok(Self::RTMP { endpoint: url }),
            "tcp" => Ok(Self::TCP {
                addr: SocketAddr::from_str(&format!(
                    "{}:{}",
                    url.host_str().unwrap(),
                    url.port().unwrap()
                ))?,
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
    fn scheme(&self) -> &'static str {
        match self {
            Self::SRT { .. } => "srt",
            Self::RTMP { .. } => "rtmp",
            Self::TCP { .. } => "tcp",
            Self::File { .. } => "file",
            Self::TestPattern => "test-pattern",
        }
    }

    pub fn to_public_url(&self, public_hostname: &str, ingest_name: &str) -> Result<String> {
        match self {
            Self::SRT { endpoint } | Self::RTMP { endpoint } => {
                let mut ep = endpoint.clone();
                ep.set_host(Some(public_hostname))
                    .map_err(|e| anyhow!("Error setting endpoint host: {}", e))?;
                ep.set_scheme(self.scheme())
                    .map_err(|_| anyhow!("Error setting endpoint scheme"))?;

                ep.set_path(ingest_name);
                Ok(ep.to_string())
            }
            Self::TCP { addr } => {
                Ok(format!("tcp://{}:{}", public_hostname, addr.port()))
            }
            _ => bail!("Unsupported endpoint scheme: {}", self.scheme()),
        }
    }
}
