//! Agent-specific logic for the session daemon: hook payloads, the status
//! state machine, launch specs, Codex hook-file merging, screen heuristics and
//! the Claude Code and Codex skill folders.
//!
//! Pure: no tokio, no network, no process spawning, no clock. The only file
//! IO is `codex_hooks::install` / `uninstall` and `skills`. Times and sequence
//! numbers come in as parameters.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod codex_hooks;
pub mod codex_trust;
pub mod heuristic;
pub mod hooks;
pub mod launch;
pub mod skills;
pub mod status;

/// The agents with hook support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}
