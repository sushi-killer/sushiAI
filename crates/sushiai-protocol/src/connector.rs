//! Exit codes of a connector process (`sushiai proxy`, `ssh`, a command profile).
//! `tests/connector-vectors/exit-codes.json` mirrors them for non-Rust clients.

/// The daemon closed the connection before the client did: retry at once.
pub const DAEMON_DIED: i32 = 2;
