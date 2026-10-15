use crate::clock::now_ms;
use crate::harness;
use crate::harness::guard::Guard;
use crate::harness::guard::Trip;
use crate::harness::watch::HookClock;
use crate::policy::variant::Variant;
use std::sync::Arc;
use std::time::Duration;

fn guarded(limit: u64) -> (Guard, Arc<HookClock>) {
    let clock = Arc::new(HookClock::default());
    let variant = Variant {
        stall_timeout_secs: limit,
        ..Default::default()
    };
    (Guard::for_variant(&variant, Some(clock.clone())), clock)
}

fn ago(secs: u64) -> tokio::time::Instant {
    tokio::time::Instant::now() - Duration::from_secs(secs)
}

#[tokio::test]
async fn helper_timed_out_uses_the_wall_deadline_even_after_output() {
    let limit = Duration::from_millis(50);
    let mut guard = Guard::helper("judge", limit);
    assert!(guard.armed());
    let deadline = guard.deadline();
    guard.heard();
    assert_eq!(guard.deadline(), deadline);
    assert!(guard.stall_expired().is_none());
    tokio::time::sleep_until(deadline).await;
    let trip = guard.stall_expired().expect("wall deadline expired");
    assert!(matches!(trip, Trip::TimedOut(_)));
    let mut outcome = harness::RunOutcome::default();
    trip.apply(&mut outcome);
    assert!(outcome.timed_out);
    assert!(!outcome.stalled && !outcome.time_budget && !outcome.over_budget);
    assert_eq!(
        outcome.error.as_deref(),
        Some("judge timed out after 0.05s")
    );

    let mut both = Guard::helper("judge", Duration::from_secs(10));
    both.stall = guarded(1).0.stall;
    assert!(both.deadline() < tokio::time::Instant::now() + Duration::from_secs(2));
}

#[test]
fn the_wall_deadline_trips_and_no_budget_is_not_armed() {
    let off = Guard::off().with_time_budget(0.0, now_ms());
    assert!(!off.armed());
    let mut fresh = Guard::off().with_time_budget(10.0, now_ms());
    assert!(fresh.armed());
    assert!(fresh.stall_expired().is_none());
    // Started 2 minutes ago with a 1 minute budget: already past.
    let mut late = Guard::off().with_time_budget(1.0, now_ms() - 120_000);
    assert!(matches!(late.stall_expired(), Some(Trip::TimeBudget(_))));
    let (stall, _) = guarded(10);
    let both = stall.with_time_budget(1.0, now_ms());
    assert!(both.deadline() <= tokio::time::Instant::now() + Duration::from_secs(10));
}

#[test]
fn a_hook_that_ends_on_the_limit_does_not_trip_the_stall() {
    let (mut guard, clock) = guarded(10);
    // Quiet since 20s ago; the hook ran the whole time and ends now,
    // exactly when the second window runs out.
    guard.last_output = ago(20);
    let hook = clock.start();
    assert!(
        guard.stall_expired().is_none(),
        "paused while the hook runs"
    );
    guard.last_output = ago(20);
    drop(hook);
    assert!(guard.stall_expired().is_none());
    assert!(guard.deadline() > tokio::time::Instant::now());
}

#[test]
fn silence_past_the_limit_after_the_hook_ends_trips() {
    let (mut guard, clock) = guarded(10);
    guard.last_output = ago(30);
    drop(clock.start());
    *clock.ended.lock().unwrap() = Some(ago(12));
    assert!(matches!(guard.stall_expired(), Some(Trip::Stalled(_))));
    *clock.ended.lock().unwrap() = Some(ago(5));
    guard.last_output = ago(30);
    assert!(
        guard.stall_expired().is_none(),
        "only 5s since the hook ended"
    );
}
