//! The orchestrator agent's conversations: several named chat sessions per
//! repository, run by the daemon so a reply keeps coming (and stays readable)
//! when the app window closes or restarts. Each turn is one CLI run that only
//! reads the repository and manages tasks through `sushiai mcp`; the harness
//! session id carries a session's conversation from turn to turn. A repo has
//! one live turn at most, and it always belongs to the `current` session.

pub(in crate::engine) mod actions;
pub(in crate::engine) mod api;
pub(in crate::engine) mod launch;
pub(in crate::engine) mod overseer;
mod prompts;
pub(in crate::engine) mod proposals;
mod reply;
mod sessions;
pub(in crate::engine) mod store;
pub(in crate::engine) mod stream;
mod tools;
pub(in crate::engine) mod turn;
mod types;
pub(in crate::engine) mod wake;

#[cfg(test)]
pub(in crate::engine) mod test_support;
