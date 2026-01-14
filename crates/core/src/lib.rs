use ffmpeg_rs_raw::ffmpeg_sys_the_third::AVCodecID;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;

pub mod egress;
pub mod endpoint;
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

#[cfg(feature = "egress-moq")]
pub use hang;

/// Maps a common codec name to a codec id in FFMPEG
pub fn map_codec_id(codec: &str) -> Option<AVCodecID> {
    match codec {
        "h264" => Some(AVCodecID::AV_CODEC_ID_H264),
        "h265" | "hevc" => Some(AVCodecID::AV_CODEC_ID_HEVC),
        "av1" => Some(AVCodecID::AV_CODEC_ID_AV1),
        "vp9" => Some(AVCodecID::AV_CODEC_ID_VP9),
        "vp8" => Some(AVCodecID::AV_CODEC_ID_VP8),
        "aac" => Some(AVCodecID::AV_CODEC_ID_AAC),
        "opus" => Some(AVCodecID::AV_CODEC_ID_OPUS),
        "webp" => Some(AVCodecID::AV_CODEC_ID_WEBP),
        _ => None,
    }
}

/// bitrate‑per‑pixel‑per‑second
pub fn recommended_bitrate(codec: &str, pixels: u64, fps: f32) -> u32 {
    let bitrate = match codec {
        "h264" => pixels as f64 * fps as f64 * 0.07,
        "h265" | "hevc" => pixels as f64 * fps as f64 * 0.035,
        "av1" => pixels as f64 * fps as f64 * 0.025,
        "vp9" => pixels as f64 * fps as f64 * 0.05,
        "webp" => pixels as f64 * fps as f64 * 0.06,
        _ => pixels as f64 * fps as f64 * 0.08,
    };
    bitrate.round() as u32
}