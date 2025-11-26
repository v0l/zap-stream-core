use sha2::{Digest, Sha256};
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;

pub mod egress;
pub mod endpoint;
pub mod frame;
mod generator;
pub mod ingress;
pub mod listen;
pub mod metrics;
pub mod mux;
pub mod overseer;
pub mod pipeline;
pub mod reorder;
#[cfg(test)]
pub mod test_hls_timing;
pub mod variant;

/// Compute SHA-256 hash of a file
pub fn hash_file_sync(f: &mut std::fs::File) -> anyhow::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buf = [0; 4096];
    f.seek(SeekFrom::Start(0))?;
    while let Ok(data) = f.read(&mut buf[..]) {
        if data == 0 {
            break;
        }
        hash.update(&buf[..data]);
    }
    let hash = hash.finalize();
    f.seek(SeekFrom::Start(0))?;
    Ok(hash.into())
}
