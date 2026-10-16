use super::{is_interrupted, PassedChecks};
use crate::task::attempt::VerifyOutcome;
use crate::verify::output::{
    timed_out_tail, CANCELLED_TAIL, INTERNAL_ERROR_PREFIX, SPAWN_FAILED_PREFIX,
};
use std::time::Duration;

fn outcome(code: Option<i32>, tail: &str) -> VerifyOutcome {
    VerifyOutcome {
        command: "check".into(),
        code,
        tail: tail.into(),
        ms: 1,
        ..Default::default()
    }
}

#[test]
fn an_outcome_is_interrupted_by_a_timeout_or_a_signal_not_by_a_test_name() {
    for result in [
        outcome(None, &timed_out_tail(Duration::from_secs(1))),
        outcome(None, ""),
        outcome(Some(124), ""),
        outcome(Some(137), ""),
        outcome(Some(143), ""),
    ] {
        assert!(is_interrupted(&result), "{result:?}");
    }
    for result in [
        outcome(Some(0), ""),
        outcome(Some(1), "Timeout is the test's name"),
        outcome(
            Some(1),
            "--- failures ---\nfailures:\n    cache::times_out_after_timeout\nTest TIMED OUT",
        ),
        outcome(
            Some(1),
            "--- failures ---\nassertion failed\n--- stdout (tail) ---\ntimeout",
        ),
        outcome(None, CANCELLED_TAIL),
        outcome(None, &format!("{SPAWN_FAILED_PREFIX}missing")),
        outcome(None, &format!("{INTERNAL_ERROR_PREFIX}pipe")),
    ] {
        assert!(!is_interrupted(&result), "{result:?}");
    }
}

#[test]
fn the_pass_cache_keeps_at_most_512_entries_and_only_exit_0() {
    let mut cache = PassedChecks::default();
    for code in [None, Some(1), Some(124), Some(137), Some(143)] {
        cache.insert("failed", "base", &outcome(code, ""));
    }
    assert!(cache.entries.is_empty());
    for i in 0..512 {
        cache.insert(&format!("tree-{i}"), "base", &outcome(Some(0), ""));
    }
    cache.insert("tree-0", "other-base", &outcome(Some(0), ""));
    cache.insert("tree-512", "base", &outcome(Some(0), ""));
    assert_eq!(cache.entries.len(), 512);
    assert_eq!(cache.order.len(), 512);
    assert!(!cache.contains("tree-0", "check", "base"));
    assert!(cache.contains("tree-1", "check", "other-base"));
    let mut sensitive = outcome(Some(0), "");
    sensitive.command = "check $ORCHD_BASE_SHA".into();
    cache.insert("sensitive", "base-a", &sensitive);
    assert!(cache.contains("sensitive", &sensitive.command, "base-a"));
    assert!(!cache.contains("sensitive", &sensitive.command, "base-b"));
}
