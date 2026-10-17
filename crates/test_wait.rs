//! The one implementation of the waits of every Rust integration and unit
//! test, shared through `#[path]` (no test-support crate): one budget per test,
//! positive waits on a real signal, absence checks over a named window.
//!
//! The budget is shared by all the waits of a test (its thread), not given to
//! each wait: a test with two waits cannot outlive the runner's own limit
//! (nextest `--profile ci` stops a test after 240 s) without a message.

#![allow(dead_code)] // each test crate uses a subset

use std::cell::Cell;
use std::time::{Duration, Instant};

/// How long an absence check watches: long enough for a daemon to act on
/// something it should not do. A positive wait never uses it.
pub const ABSENCE_WINDOW: Duration = Duration::from_millis(1500);

/// The one poll interval of every wait.
pub const POLL: Duration = Duration::from_millis(25);

thread_local! {
    static STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// The wall-clock budget of one test: 200 s (under the 240 s the CI runner
/// allows), or `SUSHIAI_TEST_DEADLINE_SECS` when it is a positive whole number.
/// A test that measures timing scales its slow job from this value.
pub fn deadline() -> Duration {
    let secs = std::env::var("SUSHIAI_TEST_DEADLINE_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0);
    Duration::from_secs(secs.unwrap_or(200))
}

/// When the budget of this test ends; the clock starts at the first call in
/// the thread.
pub fn expiry() -> Instant {
    STARTED.with(|started| {
        let start = started.get().unwrap_or_else(|| {
            let now = Instant::now();
            started.set(Some(now));
            now
        });
        start + deadline()
    })
}

/// Polls `check` until it returns `Some` or the budget ends (`None`). The
/// check runs at least once, even with the budget spent.
pub fn poll_deadline<T>(mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let end = expiry();
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// Polls `check` until it returns `Ok`. Its `Err` is the state it last
/// observed: at the end of the budget the panic prints it, then `tail()` (a
/// daemon log tail, say).
pub fn wait_state<T>(
    what: &str,
    mut check: impl FnMut() -> Result<T, String>,
    tail: impl FnOnce() -> String,
) -> T {
    let mut last = String::new();
    let found = poll_deadline(|| match check() {
        Ok(value) => Some(value),
        Err(state) => {
            last = state;
            None
        }
    });
    found.unwrap_or_else(|| {
        let state = if last.is_empty() {
            String::new()
        } else {
            format!("; last state: {last}")
        };
        let tail = tail();
        let tail = if tail.is_empty() {
            tail
        } else {
            format!("\n{tail}")
        };
        panic!(
            "timed out after the {:?} test budget waiting for {what}{state}{tail}",
            deadline()
        )
    })
}

/// Polls `check` until it returns `Some`; panics naming `what` at the end of
/// the budget.
pub fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    wait_state(what, || check().ok_or_else(String::new), String::new)
}

/// Polls `ok` until it is true; panics naming `what` at the end of the budget.
pub fn wait_flag(what: &str, mut ok: impl FnMut() -> bool) {
    wait_until(what, || ok().then_some(()));
}

/// An absence check: polls `still` for `window` and panics naming `what` as
/// soon as it stops holding. This, not a bare sleep, is how a test waits for
/// something not to happen.
pub fn holds_for(what: &str, window: Duration, mut still: impl FnMut() -> bool) {
    let end = Instant::now() + window;
    loop {
        assert!(still(), "{what} stopped holding");
        if Instant::now() >= end {
            return;
        }
        std::thread::sleep(POLL);
    }
}
