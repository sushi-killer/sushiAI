//! Shared black-box fixtures: process ownership, daemon transport, repositories and fake harnesses.
#![allow(dead_code)]

pub mod attempts;
pub mod harness;
#[path = "../support/native_sandbox.rs"]
pub mod native_sandbox;
pub mod polling;
pub mod process;
pub mod protocol;
pub mod repository;
pub mod sandbox;
