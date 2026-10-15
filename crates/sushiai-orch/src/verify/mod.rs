//! Verification and repository facts used by planning, recovery and checks.

pub(crate) mod repository;

pub mod evidence;

pub(crate) mod base_cache;
pub mod command;
pub(crate) mod comparison;
pub(crate) mod diff;
pub(crate) mod healing;
pub(crate) mod load;
pub(crate) mod options;
pub(crate) mod output;
pub(crate) mod passes;
pub mod review;
pub(crate) mod scoping;
