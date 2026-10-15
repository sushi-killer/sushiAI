//! Scoped JSON-RPC bridge used by native agent harnesses.

mod call;
mod schema;
mod scope;
mod session;

pub use schema::{ORCHESTRATOR_TOOLS, READ_ONLY_TOOLS, TASK_TOOLS};
pub use scope::Caller;
pub use session::serve_stdio;

#[cfg(test)]
mod answer_source_tests;
#[cfg(test)]
mod tests;
