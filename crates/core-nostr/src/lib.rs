pub mod blossom;
pub mod n94;

use sha2::{Digest, Sha256};
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Compute SHA-256 hash of a file
pub(crate) async fn hash_file(f: &mut tokio::fs::File) -> anyhow::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buf = [0; 4096];
    f.seek(SeekFrom::Start(0)).await?;
    while let Ok(data) = f.read(&mut buf[..]).await {
        if data == 0 {
            break;
        }
        hash.update(&buf[..data]);
    }
    let hash = hash.finalize();
    f.seek(SeekFrom::Start(0)).await?;
    Ok(hash.into())
}
