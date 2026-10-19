//! Agent-specific logic for the session daemon: the registry of agent CLIs,
//! hook payloads, the status state machine, launch specs, Codex hook-file
//! merging and the Claude Code and Codex skill folders.
//!
//! Pure: no tokio, no network, no process spawning, no clock. The only file
//! IO is `codex_hooks::install` / `uninstall`, `skills` and `skill_usage`. Times and sequence
//! numbers come in as parameters.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod codex_hooks;
pub mod codex_trust;
pub mod harness;
pub mod hooks;
pub mod launch;
pub mod skill_usage;
pub mod skills;
pub mod status;
