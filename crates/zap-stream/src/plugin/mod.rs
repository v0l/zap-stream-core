#[cfg(feature = "chromaprint")]
mod track_id;
#[cfg(feature = "chromaprint")]
pub use track_id::*;

/// A music track id match
#[derive(Clone, Debug)]
pub struct TrackIdMatch {
    pub id: String,
}
