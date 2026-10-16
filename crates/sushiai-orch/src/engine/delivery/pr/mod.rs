pub(crate) mod carry;
pub(in crate::engine) mod ci_watch;
pub(crate) mod conflict;
pub(crate) mod draft;
pub(crate) mod helper;
pub(crate) mod open;
pub(crate) mod pending;
pub(crate) mod publish;

#[cfg(test)]
#[path = "tests/pr_refusal_tests.rs"]
mod pr_refusal_tests;

pub(crate) mod automatic_carry;
pub(crate) mod carried_work;
pub(crate) mod checkout;
pub(crate) mod plan;
#[cfg(test)]
#[path = "tests/pr_draft_tests.rs"]
mod pr_draft_tests;
pub(crate) mod prepare;
pub(crate) mod record;
pub(crate) mod remote;
pub(crate) mod retry;

mod validation;

mod observations;
pub(in crate::engine) mod repair;

pub(in crate::engine) mod admission;

mod resolution;
