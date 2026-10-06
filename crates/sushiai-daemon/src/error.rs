#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame: {0}")]
    Frame(#[from] sushiai_protocol::FrameError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("state file: {0}")]
    State(#[from] sushiai_core::StateError),
    #[error("a daemon is already running on {0}")]
    AlreadyRunning(String),
    #[error("{0} is not safe to use: it must be a directory (not a symlink) owned by you")]
    UnsafeDir(String),
    #[error("holder: {0}")]
    Holder(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A JSON-RPC error: stable code and a message.
pub type Fail = (i64, String);
