use crate::egress::EgressSegment;
use crate::mux::SegmentType;
use crate::mux::hls::variant::HlsVariant;
use chrono::{DateTime, Utc};
use m3u8_rs::{ByteRange, MediaSegment, MediaSegmentType, Part};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(PartialEq)]
pub enum HlsSegment {
    Full(SegmentInfo),
    Partial(PartialSegmentInfo),
}

impl HlsSegment {
    pub fn to_media_segment(&self) -> MediaSegmentType {
        match self {
            HlsSegment::Full(f) => f.to_media_segment(),
            HlsSegment::Partial(p) => p.to_media_segment(),
        }
    }

    /// Convert to EgressSegment with variant ID and path
    pub fn to_egress_segment(&self, variant_id: Uuid, path: PathBuf) -> Option<EgressSegment> {
        match self {
            HlsSegment::Full(seg) => Some(EgressSegment {
                variant: variant_id,
                idx: seg.index,
                duration: seg.duration,
                path,
                sha256: seg.sha256,
            }),
            HlsSegment::Partial(_) => None, // Partial segments don't have full segment info
        }
    }
}

#[derive(PartialEq)]
pub struct SegmentInfo {
    pub index: u64,
    pub duration: f32,
    pub kind: SegmentType,
    pub sha256: [u8; 32],
    pub timestamp: DateTime<Utc>,
    pub discontinuity: bool,
}

impl SegmentInfo {
    pub fn to_media_segment(&self) -> MediaSegmentType {
        MediaSegmentType::Full(MediaSegment {
            uri: self.filename(),
            duration: self.duration,
            program_date_time: Some(self.timestamp.fixed_offset()),
            discontinuity: self.discontinuity,
            ..MediaSegment::default()
        })
    }

    pub fn filename(&self) -> String {
        HlsVariant::segment_name(self.kind, self.index)
    }
}

#[derive(PartialEq)]
pub struct PartialSegmentInfo {
    pub index: u64,
    pub parent_index: u64,
    pub parent_kind: SegmentType,
    pub duration: f64,
    pub independent: bool,
    pub byte_range: Option<(u64, Option<u64>)>,
}

impl PartialSegmentInfo {
    pub fn to_media_segment(&self) -> MediaSegmentType {
        MediaSegmentType::Partial(Part {
            uri: self.filename(),
            duration: self.duration,
            independent: self.independent,
            gap: false,
            byte_range: self.byte_range.map(|r| ByteRange {
                length: r.0,
                offset: r.1,
            }),
        })
    }

    pub fn filename(&self) -> String {
        HlsVariant::segment_name(self.parent_kind, self.parent_index)
    }

    /// Byte offset where this partial segment ends
    pub fn end_pos(&self) -> Option<u64> {
        self.byte_range
            .as_ref()
            .map(|(len, start)| start.unwrap_or(0) + len)
    }
}
