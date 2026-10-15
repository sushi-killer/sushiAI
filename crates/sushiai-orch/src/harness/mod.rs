//! Argv construction for `claude`/`codex`, and parsing of their streamed
//! JSON events. Argv building is pure (`Vec<String>` in, testable without a
//! process); stream parsing folds one line at a time into a
//! [`RunOutcome`] as the engine reads the child's stdout.

mod claude;
mod codex;
mod failure;
mod fingerprint;
mod outcome;
mod request;
mod stream;

pub use claude::{
    build_claude_settings, build_claude_task_settings, claude_argv, shell_quote, StopHook,
    DELEGATION_TOOLS, DENIED_BASH_COMMANDS, HOOKED_TOOLS, HOOK_HOLD_SECS,
};
pub(crate) use codex::task_run_uses_outer_sandbox;
pub use codex::{codex_argv, codex_mcp_flags};
pub use failure::ProviderFailure;
pub use fingerprint::prompt_hash;
pub use outcome::{exit_error, MessageUsage, PermissionDenial, RunOutcome};
pub use request::{build_argv, build_argv_with_schema, OutputSchema, RunRequest};
pub use stream::{feed_stream_line, replay_events};

#[cfg(test)]
mod tests;

pub(crate) mod process;

pub(crate) mod run_files;

pub(crate) mod environment;

pub(crate) mod cancel;
pub(crate) mod watch;

pub mod compiler_cache;
pub(crate) mod guard;
pub(crate) mod native;
