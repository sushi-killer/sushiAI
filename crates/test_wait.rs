//! The waits of the small crates' integration tests, shared through `#[path]`
//! (no test-support crate). `sushiai` and `sushiai-orch` have the same
//! contract in their own `tests/common`: one deadline, positive waits on a
//! real signal, absence checks over a named window.

#![allow(dead_code)] // each test crate uses a subset

use std::time::{Duration, Instant};

/// The one wall-clock budget of a positive wait: 180 s, or
/// `SUSHIAI_TEST_DEADLINE_SECS` when it is a positive whole number.
pub fn deadline() -> Duration {
    let secs = std::env::var("SUSHIAI_TEST_DEADLINE_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0);
    Duration::from_secs(secs.unwrap_or(180))
}

/// Polls `check` until it returns `Some`; panics naming `what` at the deadline.
pub fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + deadline();
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An absence check: polls `still` for `window` and panics naming `what` as
/// soon as it stops holding.
pub fn holds_for(what: &str, window: Duration, mut still: impl FnMut() -> bool) {
    let end = Instant::now() + window;
    loop {
        assert!(still(), "{what} stopped holding");
        if Instant::now() >= end {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
