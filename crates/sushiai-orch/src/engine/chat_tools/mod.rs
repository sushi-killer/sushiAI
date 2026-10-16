//! Connected tools of the orchestrator chat: MCP servers the owner toggles in
//! Settings. A server definition is read at turn time from where the owner
//! already keeps it (`~/.claude.json`, an installed Claude Code plugin), so no
//! secret is copied into orchd's settings. Read tools are handed to the turn;
//! a write tool is never callable by the model: it proposes the call in a
//! `sushi-action` block and the owner's OK runs exactly that call.

pub mod access;
pub mod approved;
pub(in crate::engine) mod catalog;
pub mod discovery;
pub mod server;
pub mod status;
mod stdio;
pub mod turn;
