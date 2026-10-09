//! Pure session logic: screen model, session transitions, state file model.
//! No IO and no async runtime.

pub mod catalog;
pub mod hibernate;
mod screen;
mod session;
mod state;

pub use screen::{Screen, SCROLLBACK_LINES};
pub use session::{mark_exited, mark_hibernated};
pub use state::{StateError, StateFile, SCHEMA_VERSION};
