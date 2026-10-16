//! An implementer's claim that a protected planner test is wrong. The test is
//! restored whatever the claim says; a fresh read-only judge on the planner's
//! route checks the claim against the task goal. Confirmed: the planner (not
//! the implementer) rewrites the test and orchd commits it into the planner
//! tests. Rejected: the agent carries on. Unclear: one owner question.

pub(in crate::engine) mod brief;
mod checkout;
mod planner;
mod reply;
mod rewrite;
pub(in crate::engine) mod run;

#[cfg(test)]
mod tests;
