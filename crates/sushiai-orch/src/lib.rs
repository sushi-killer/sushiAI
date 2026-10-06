//! sushiAI task orchestration pipeline (was the `orchd` daemon).

pub mod ab;
pub mod brief;
pub mod cli;
pub mod costs;
pub mod engine;
pub mod eval;
pub mod evolve;
pub mod git;
pub mod harness;
pub mod hook;
pub mod loop_detect;
pub mod mcp;
pub mod model;
pub mod prompts;
pub mod protocol;
pub mod report;
pub mod skill;
pub mod store;
pub mod timeline;
