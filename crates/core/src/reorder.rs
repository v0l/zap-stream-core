use std::collections::BinaryHeap;

/// Maximum number of frames to buffer for reordering before forcing output
const FRAME_REORDER_BUFFER_SIZE: usize = 16;

/// Minimum number of frames to buffer before starting to emit.
/// This provides lookahead for the encoder to properly assign DTS with B-frames.
/// Typically max_b_frames + 1 is sufficient.
const MIN_BUFFER_DEPTH: usize = 4;

/// A frame wrapper that can be ordered by PTS for the reorder buffer.
/// Uses Reverse to create a min-heap (lowest PTS first).
struct PtsOrderable<T> {
    pts: i64,
    duration: i64,
    value: T,
}

impl<T> Ord for PtsOrderable<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse ordering so BinaryHeap becomes a min-heap (lowest PTS pops first)
        other.pts.cmp(&self.pts)
    }
}

impl<T> PartialOrd for PtsOrderable<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for PtsOrderable<T> {
    fn eq(&self, other: &Self) -> bool {
        self.pts == other.pts
    }
}

impl<T> Eq for PtsOrderable<T> {}

/// Buffer that reorders video frames by PTS before sending to encoder.
/// This is necessary because decoders may output frames in decode order (DTS)
/// rather than presentation order (PTS), especially with B-frames.
pub struct FrameReorderBuffer<T> {
    heap: BinaryHeap<PtsOrderable<T>>,
    max_size: usize,
    min_depth: usize,
    /// The next expected PTS value (pts + duration of last emitted frame)
    next_pts: Option<i64>,
}

impl<T> FrameReorderBuffer<T> {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::with_capacity(FRAME_REORDER_BUFFER_SIZE + 1),
            max_size: FRAME_REORDER_BUFFER_SIZE,
            min_depth: MIN_BUFFER_DEPTH,
            next_pts: None,
        }
    }

    /// Push a frame into the buffer, taking ownership.
    /// Returns frames that should be processed (in PTS order) when they form a contiguous sequence.
    pub fn push(&mut self, pts: i64, duration: i64, value: T) -> Vec<T> {
        self.heap.push(PtsOrderable {
            pts,
            duration,
            value,
        });
        let mut output = Vec::new();

        // Don't emit until we have minimum buffer depth for encoder lookahead
        if self.heap.len() < self.min_depth {
            return output;
        }

        // Pop frames that are contiguous with what we've already emitted
        loop {
            // Keep at least min_depth frames buffered for encoder lookahead
            if self.heap.len() <= self.min_depth {
                break;
            }

            let Some(next) = self.heap.peek() else {
                break;
            };

            // If this is the first frame, or the frame's PTS matches our expected next PTS
            let should_emit = match self.next_pts {
                None => true, // First frame, emit it
                Some(expected) => next.pts <= expected, // Frame is at or before expected PTS
            };

            if should_emit {
                let ordered = self.heap.pop().unwrap();
                // Update next_pts to be this frame's pts + duration
                self.next_pts = Some(ordered.pts + ordered.duration);
                output.push(ordered.value);
            } else {
                break;
            }
        }

        // Safety valve: if buffer is too full, force emit oldest frames
        while self.heap.len() > self.max_size {
            if let Some(ordered) = self.heap.pop() {
                self.next_pts = Some(ordered.pts + ordered.duration);
                output.push(ordered.value);
            }
        }

        output
    }

    /// Flush all remaining frames from the buffer in PTS order.
    #[allow(dead_code)]
    pub fn flush(&mut self) -> Vec<T> {
        let mut output = Vec::with_capacity(self.heap.len());
        while let Some(ordered) = self.heap.pop() {
            output.push(ordered.value);
        }
        self.next_pts = None;
        output
    }

    /// Check if buffer is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}
