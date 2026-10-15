//! sushiAI task orchestration pipeline (was the `orchd` daemon). It has no process, socket or
//! lock of its own: a host starts [`Orch`], forwards `orch.*` calls to [`Orch::call`] and sends
//! every value passed to the `emit` callback to its clients as `orch.event`.

pub mod ab;
pub mod costs;
pub mod delivery;
pub mod engine;
pub mod eval;
pub mod evolve;
pub mod git;
pub mod harness;
pub mod legacy;
pub mod loop_detect;
pub mod overseer;
pub mod overseer_state;
pub mod policy;
pub mod precedents;
#[path = "../../process.rs"]
pub mod process;
pub mod procs;
pub mod prompts;
pub mod report;
pub mod sandbox;
pub mod skill;
pub mod store;
pub mod task;
pub mod test_failures;
pub mod timeline;
pub mod verify;
pub mod worktree_env;

pub mod api;
pub use api::{CallError, Config};
pub use engine::runtime::{CallFuture, Orch};

pub mod clock;
