use anyhow::{Result, ensure};
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum_extra::body::AsyncReadBody;
use http_range_header::{EndPosition, StartPosition, SyntacticallyCorrectRange};
use std::io::SeekFrom;
use std::ops::Range;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tracing::warn;

/// Range request handler over file handle
pub struct RangeBody {
    file: File,
    current_offset: u64,
    poll_complete: bool,
    file_size: u64,
    range: Option<Range<u64>>,
}

const MAX_UNBOUNDED_RANGE: u64 = 1024 * 1024;
impl RangeBody {
    pub fn new(file: File, file_size: u64) -> Self {
        Self {
            file,
            file_size,
            range: None,
            current_offset: 0,
            poll_complete: false,
        }
    }

    pub fn with_range(mut self, range: Range<u64>) -> Self {
        self.range = Some(range);
        self
    }

    pub fn get_range(file_size: u64, header: &SyntacticallyCorrectRange) -> Result<Range<u64>> {
        let range_start = match header.start {
            StartPosition::Index(i) => {
                ensure!(i < file_size, "Range start out of range");
                i
            }
            StartPosition::FromLast(i) => file_size.saturating_sub(i),
        };
        let range_end = match header.end {
            EndPosition::Index(i) => {
                ensure!(i <= file_size, "Range end out of range");
                i
            }
            EndPosition::LastByte => {
                (file_size.saturating_sub(1)).min(range_start + MAX_UNBOUNDED_RANGE)
            }
        };
        Ok(range_start..range_end)
    }
}

impl IntoResponse for RangeBody {
    fn into_response(self) -> Response {
        let mut rsp = Response::builder().status(StatusCode::OK);
        if let Some(range) = &self.range {
            let r_len = (range.end - range.start) + 1;
            rsp = rsp
                .status(StatusCode::PARTIAL_CONTENT)
                .header("content-length", r_len)
                .header(
                    "content-range",
                    format!("bytes {}-{}/{}", range.start, range.end, self.file_size),
                );
        }
        match rsp.body(Body::new(AsyncReadBody::new(self))) {
            Ok(r) => r,
            Err(e) => {
                warn!("Error creating range body: {:?}", e);
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap()
            }
        }
    }
}

impl AsyncRead for RangeBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let range_start = self.range.as_ref().map(|r| r.start).unwrap_or(0) + self.current_offset;
        let range_len = self
            .range
            .as_ref()
            .map(|r| r.end)
            .unwrap_or(self.file_size - 1)
            .saturating_sub(range_start)
            + 1;
        let bytes_to_read = buf.remaining().min(range_len as usize) as u64;

        if bytes_to_read == 0 {
            return Poll::Ready(Ok(()));
        }

        // when no pending poll, seek to starting position
        if !self.poll_complete {
            let pinned = pin!(&mut self.file);
            pinned.start_seek(SeekFrom::Start(range_start))?;
            self.poll_complete = true;
        }

        // check poll completion
        if self.poll_complete {
            let pinned = pin!(&mut self.file);
            match pinned.poll_complete(cx) {
                Poll::Ready(Ok(_)) => {
                    self.poll_complete = false;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }

        // Read data from the file
        let pinned = pin!(&mut self.file);
        match pinned.poll_read(cx, buf) {
            Poll::Ready(Ok(_)) => {
                self.current_offset += bytes_to_read;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                self.poll_complete = true;
                Poll::Pending
            }
        }
    }
}
