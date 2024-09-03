use serde::{Deserialize, Serialize};

pub mod file;
pub mod srt;
pub mod tcp;
pub mod test;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Endpoint of the ingress
    pub endpoint: String,

    /// IP address of the connection
    pub ip_addr: String,
}
