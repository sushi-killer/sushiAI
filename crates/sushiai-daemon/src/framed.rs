use std::collections::VecDeque;

use sushiai_protocol::{encode, Decoder, Frame};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::Result;

/// Reads frames from a stream. `next` is cancel-safe: partial input stays in the reader.
pub struct FrameReader<R> {
    inner: R,
    decoder: Decoder,
    queue: VecDeque<Frame>,
    buf: Box<[u8; 16384]>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        FrameReader {
            inner,
            decoder: Decoder::new(),
            queue: VecDeque::new(),
            buf: Box::new([0; 16384]),
        }
    }

    /// `None` at end of stream.
    pub async fn next(&mut self) -> Result<Option<Frame>> {
        loop {
            if let Some(frame) = self.queue.pop_front() {
                return Ok(Some(frame));
            }
            let n = self.inner.read(&mut self.buf[..]).await?;
            if n == 0 {
                return Ok(None);
            }
            self.queue.extend(self.decoder.push(&self.buf[..n])?);
        }
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(out: &mut W, frame: &Frame) -> Result<()> {
    out.write_all(&encode(frame)).await?;
    Ok(())
}
