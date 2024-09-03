use std::fmt::{Display, Formatter};

use ffmpeg_sys_next::AVStream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::variant::StreamMapping;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct VariantMapping {
    /// Unique ID of this variant
    pub id: Uuid,

    /// Source video stream to use for this variant
    pub src_index: usize,

    /// Index of this variant stream in the output
    pub dst_index: usize,

    /// Stream group, groups one or more streams into a variant
    pub group_id: usize,
}

impl Display for VariantMapping {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Copy #{}->{}", self.src_index, self.dst_index)
    }
}

impl StreamMapping for VariantMapping {
    fn id(&self) -> Uuid {
        self.id
    }

    fn src_index(&self) -> usize {
        self.src_index
    }

    fn dst_index(&self) -> usize {
        self.dst_index
    }

    fn set_dst_index(&mut self, dst: usize) {
        self.dst_index = dst;
    }

    fn group_id(&self) -> usize {
        self.group_id
    }

    unsafe fn to_stream(&self, stream: *mut AVStream) {
        // do nothing
    }
}
