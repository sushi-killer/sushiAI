//! Detect completed-task evidence and run reviewed evolution proposals.

mod clustering;
pub mod detection;
mod evidence;
mod job;
mod measurement;
mod native;
mod propose;
mod record;
mod rpc;
#[cfg(test)]
mod test_support;
