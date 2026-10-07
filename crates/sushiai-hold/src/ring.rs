use std::collections::VecDeque;

/// Keeps the tail of the output stream. `base` is the stream offset of the first kept byte.
pub struct Ring {
    buf: VecDeque<u8>,
    base: u64,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Ring {
            buf: VecDeque::new(),
            base: 0,
            cap,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend(bytes);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }

    /// Offset one past the last byte.
    pub fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    /// Bytes from `max(seq, base)` to the end, with their start offset.
    pub fn read_from(&self, seq: u64) -> (u64, Vec<u8>) {
        let start = seq.clamp(self.base, self.end());
        let skip = (start - self.base) as usize;
        (start, self.buf.iter().skip(skip).copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

    #[test]
    fn keeps_the_tail_and_tracks_base() {
        let mut ring = Ring::new(4);
        ring.push(b"abc");
        assert_eq!(ring.read_from(0), (0, b"abc".to_vec()));
        ring.push(b"defg");
        assert_eq!(ring.end(), 7);
        assert_eq!(ring.read_from(0), (3, b"defg".to_vec()));
        assert_eq!(ring.read_from(5), (5, b"fg".to_vec()));
        assert_eq!(ring.read_from(99), (7, Vec::new()));
    }
}
