//! Lifecycle, concurrency and persistence coordination.

pub mod audit;
pub(crate) mod chat;
#[path = "../evolution/mod.rs"]
pub mod evolution;
#[path = "../messages.rs"]
mod messages;

mod advisor;
pub(crate) mod answer_policy;

mod attribution;
mod availability;
mod backlog;
mod baseline;
mod brief_check;
mod chat_tools;
mod cost;
mod decision;
mod decision_memory;
mod deliverables;
mod digest;
mod finding_judge;
mod fresh_base;
mod graph;
mod hooks;
mod ladder;
mod land;
mod leases;
mod lines;
mod metrics;
pub(crate) mod owner_notice;
mod park;
mod past_work;
mod permissions;
mod pipeline;
mod plan_gate;
mod protected;
mod questions;
mod rate_limit;
mod recovery;
mod repo_notes;
mod reports;
mod review;
mod review_findings;
mod routing;
mod rpc;
mod rpc_misc;
mod secrets;
mod slots;
mod stuck;
mod subtask_checks;
mod sync;
mod test_claim;
#[cfg(test)]
pub(crate) mod test_support;
mod time_budget;
mod wait_sla;
mod worktrees;

pub mod attempts;

pub mod planning;

pub mod runtime;

pub mod host_limits;

pub(crate) mod run_control;

pub(crate) mod state;

pub(crate) mod persistence;

pub(crate) mod timeouts;

pub(crate) mod sandbox;

pub(crate) mod shutdown;

pub(crate) mod events;

pub(crate) mod dispatch;

#[cfg(test)]
#[path = "tests/foundation.rs"]
pub(crate) mod tests;

pub(crate) mod lifecycle;

mod delivery_gate;

pub(crate) mod verification;

pub(crate) mod delivery;
