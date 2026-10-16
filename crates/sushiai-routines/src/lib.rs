//! Routines: a folder (`routine.json`, `brief.md`) the owner's host runs on a schedule as a
//! background agent session that ends in a schema-valid JSON result.
//!
//! The crate is pure logic plus the file store. It reaches the daemon only through the
//! [`Connector`] trait and runs git only through [`Git`]; the `sushiai` binary supplies both.
//! No sushiai core crate is a dependency.

pub mod argv;
pub mod cron;
pub mod help;
pub mod model;
pub mod result;
pub mod service;
pub mod store;
pub mod validate;

pub use model::{Access, Agent, HowItRuns, Limits, Routine, When, Where};
pub use service::{CallError, Config, Connector, Git, Routines};
