//! Frame codec: `u32` BE length of (kind + payload), `u8` kind, payload.

pub const MAX_FRAME: usize = 16 * 1024 * 1024;
const KIND_JSON: u8 = b'J';
const KIND_OUTPUT: u8 = b'B';

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// One UTF-8 JSON-RPC 2.0 message.
    Json(String),
    /// Raw terminal output. `seq` is the offset of `data[0]` in the session's output stream.
    Output { id: String, seq: u64, data: Vec<u8> },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME} byte limit")]
    TooLarge(usize),
    #[error("empty frame")]
    Empty,
    #[error("unknown frame kind {0:#04x}")]
    UnknownKind(u8),
    #[error("truncated output frame")]
    Truncated,
    #[error("JSON frame is not valid UTF-8")]
    InvalidUtf8,
}

/// Encodes one frame. The session id of an output frame must fit in `u16`.
pub fn encode(frame: &Frame) -> Vec<u8> {
    let (kind, body) = match frame {
        Frame::Json(text) => (KIND_JSON, text.as_bytes().to_vec()),
        Frame::Output { id, seq, data } => {
            debug_assert!(id.len() <= usize::from(u16::MAX));
            let mut body = Vec::with_capacity(10 + id.len() + data.len());
            body.extend_from_slice(&(id.len() as u16).to_be_bytes());
            body.extend_from_slice(id.as_bytes());
            body.extend_from_slice(&seq.to_be_bytes());
            body.extend_from_slice(data);
            (KIND_OUTPUT, body)
        }
    };
    let mut out = Vec::with_capacity(5 + body.len());
    out.extend_from_slice(&((body.len() + 1) as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(&body);
    out
}

/// Incremental decoder: feed any slice of the byte stream, get the frames completed so far.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, FrameError> {
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut at = 0;
        while self.buf.len() - at >= 4 {
            let mut len = [0u8; 4];
            len.copy_from_slice(&self.buf[at..at + 4]);
            let len = u32::from_be_bytes(len) as usize;
            if len > MAX_FRAME {
                return Err(FrameError::TooLarge(len));
            }
            if len == 0 {
                return Err(FrameError::Empty);
            }
            if self.buf.len() - at - 4 < len {
                break;
            }
            frames.push(parse(&self.buf[at + 4..at + 4 + len])?);
            at += 4 + len;
        }
        self.buf.drain(..at);
        Ok(frames)
    }
}

fn parse(body: &[u8]) -> Result<Frame, FrameError> {
    let (kind, payload) = (body[0], &body[1..]);
    match kind {
        KIND_JSON => String::from_utf8(payload.to_vec())
            .map(Frame::Json)
            .map_err(|_| FrameError::InvalidUtf8),
        KIND_OUTPUT => {
            let id_len = match payload {
                [a, b, ..] => usize::from(u16::from_be_bytes([*a, *b])),
                _ => return Err(FrameError::Truncated),
            };
            let rest = &payload[2..];
            if rest.len() < id_len + 8 {
                return Err(FrameError::Truncated);
            }
            let id =
                String::from_utf8(rest[..id_len].to_vec()).map_err(|_| FrameError::InvalidUtf8)?;
            let mut seq = [0u8; 8];
            seq.copy_from_slice(&rest[id_len..id_len + 8]);
            Ok(Frame::Output {
                id,
                seq: u64::from_be_bytes(seq),
                data: rest[id_len + 8..].to_vec(),
            })
        }
        other => Err(FrameError::UnknownKind(other)),
    }
}
