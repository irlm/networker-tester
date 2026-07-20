//! Response bodies (contract v1 §3.3): a streamed download body that emits the
//! fill byte from a single per-process buffer, never allocating memory
//! proportional to the requested size.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};

use crate::config::{DOWNLOAD_CHUNK_BYTES, DOWNLOAD_FILL};

/// A per-process, read-only chunk of [`DOWNLOAD_FILL`] bytes. Every download
/// response slices `Bytes` out of this single `Arc` — no per-request buffer.
#[derive(Clone)]
pub struct FillBuffer(Arc<Bytes>);

impl FillBuffer {
    pub fn new() -> Self {
        FillBuffer(Arc::new(Bytes::from(vec![
            DOWNLOAD_FILL;
            DOWNLOAD_CHUNK_BYTES
        ])))
    }

    /// Build a streaming download body of exactly `total` bytes.
    pub fn body(&self, total: u64) -> DownloadBody {
        DownloadBody {
            chunk: (*self.0).clone(),
            remaining: total,
        }
    }
}

impl Default for FillBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// A [`http_body::Body`] that emits `remaining` bytes of fill in chunks of at
/// most [`DOWNLOAD_CHUNK_BYTES`], slicing from the shared buffer. Peak memory
/// is O(chunk), independent of `remaining`.
pub struct DownloadBody {
    chunk: Bytes,
    remaining: u64,
}

impl Body for DownloadBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.remaining == 0 {
            return Poll::Ready(None);
        }
        let take = this.remaining.min(this.chunk.len() as u64) as usize;
        this.remaining -= take as u64;
        // slice() is a zero-copy view into the shared Arc buffer.
        let frame = this.chunk.slice(0..take);
        Poll::Ready(Some(Ok(Frame::data(frame))))
    }

    fn is_end_stream(&self) -> bool {
        self.remaining == 0
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn streams_exact_byte_count_of_fill() {
        let fill = FillBuffer::new();
        let total = DOWNLOAD_CHUNK_BYTES as u64 * 2 + 7;
        let collected = fill.body(total).collect().await.unwrap().to_bytes();
        assert_eq!(collected.len() as u64, total);
        assert!(collected.iter().all(|&b| b == DOWNLOAD_FILL));
    }

    #[test]
    fn size_hint_is_exact() {
        let fill = FillBuffer::new();
        assert_eq!(fill.body(1234).size_hint().exact(), Some(1234));
    }
}
