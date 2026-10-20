//! Agent-specific logic for the session daemon: the registry of agent CLIs and
//! their launch argv, hook payloads, the status state machine, Codex hook-file
//! merging and trust, and the Claude Code and Codex skill folders.
//!
//! Pure: no tokio, no network, no process spawning, no clock. The only file
//! IO is `file`, `harness::codex::hooks`, `skills` and `skill_usage`. Times and
//! sequence numbers come in as parameters.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod file;
pub mod harness;
pub mod hooks;
pub mod skill_usage;
pub mod skills;
pub mod status;
