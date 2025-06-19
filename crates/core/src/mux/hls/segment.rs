use crate::mux::hls::variant::HlsVariant;
use crate::mux::SegmentType;
use m3u8_rs::{ByteRange, MediaSegment, MediaSegmentType, Part};

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
}

#[derive(PartialEq)]
pub struct SegmentInfo {
    pub index: u64,
    pub duration: f32,
    pub kind: SegmentType,
}

impl SegmentInfo {
    pub fn to_media_segment(&self) -> MediaSegmentType {
        MediaSegmentType::Full(MediaSegment {
            uri: self.filename(),
            duration: self.duration,
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
