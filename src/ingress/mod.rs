use serde::{Deserialize, Serialize};

pub mod srt;
pub mod tcp;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Endpoint of the ingress
    pub endpoint: String,

    /// IP address of the connection
    pub ip_addr: String,
}