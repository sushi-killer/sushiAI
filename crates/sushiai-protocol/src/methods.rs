//! Method names, error codes and typed params/results.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CAPABILITIES: &[&str] = &["sessions", "attach", "resync"];

pub mod method {
    pub const HELLO: &str = "hello";
    pub const SESSION_CREATE: &str = "session.create";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_INPUT: &str = "session.input";
    pub const SESSION_RESIZE: &str = "session.resize";
    pub const SESSION_CLOSE: &str = "session.close";
    pub const SESSION_ATTACH: &str = "session.attach";
    pub const SESSION_DETACH: &str = "session.detach";
    pub const SESSION_EXITED: &str = "session.exited";
    /// Notification sent to a subscriber that fell behind: a fresh snapshot replaces the lost bytes.
    pub const SESSION_SNAPSHOT: &str = "session.snapshot";
    /// Notification sent to a client that missed events: the full session list.
    pub const SESSION_RESYNC: &str = "session.resync";
    pub const HOLD_ATTACH: &str = "hold.attach";
    pub const HOLD_INPUT: &str = "hold.input";
    pub const HOLD_RESIZE: &str = "hold.resize";
    pub const HOLD_CLOSE: &str = "hold.close";
    pub const HOLD_EXITED: &str = "hold.exited";
}

pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const NOT_INITIALIZED: i64 = 1001;
    pub const PROTOCOL_MISMATCH: i64 = 1002;
    pub const SESSION_NOT_FOUND: i64 = 1003;
    pub const SPAWN_FAILED: i64 = 1004;
    pub const SESSION_NOT_RUNNING: i64 = 1005;
    /// The input queue of a session is full; the child is not reading.
    pub const INPUT_BACKPRESSURE: i64 = 1006;
}

/// Serde adapter: `Vec<u8>` as a standard base64 string.
pub mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub client: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub capabilities: Vec<String>,
    pub daemon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreate {
    pub cmd: Vec<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionId {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    /// The daemon lost its holder connection; the session may still run. Never persisted.
    Detached,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub title: Option<String>,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Pid of the session's holder, so a dead holder can be told from a busy one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInput {
    pub id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResize {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClose {
    pub id: String,
    pub graceful: bool,
}

/// Result of `session.attach`; also the params of a `session.snapshot` notification (with `id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachResult {
    #[serde(with = "b64")]
    pub snapshot: Vec<u8>,
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    #[serde(flatten)]
    pub attach: AttachResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsResync {
    pub sessions: Vec<SessionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExited {
    pub id: String,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldAttach {
    pub from_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldAttachResult {
    pub from_seq: u64,
    pub next_seq: u64,
    #[serde(default)]
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldInput {
    #[serde(with = "b64")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldResize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    #[serde(rename = "TERM")]
    Term,
    #[serde(rename = "KILL")]
    Kill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldClose {
    pub signal: Signal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldExited {
    pub code: Option<i32>,
}
