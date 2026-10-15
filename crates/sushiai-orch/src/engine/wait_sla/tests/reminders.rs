use crate::engine::test_support::test_app;
use crate::engine::wait_sla::tests::support::calls_reach;
use crate::engine::wait_sla::tests::support::held_flags;
use crate::engine::wait_sla::tests::support::waiting_in;
use crate::engine::wait_sla::tests::support::HALF_HOUR;
use crate::engine::wait_sla::tests::support::T0;
use crate::policy::waits::default_due;
use crate::policy::waits::reminder_due;
use crate::policy::waits::HOUR_MS;
use crate::task::QuestionKind;

#[test]
fn a_reminder_is_due_one_interval_after_the_question_or_the_last_reminder() {
    assert!(!reminder_due(T0, None, 0, T0 + 99 * HOUR_MS));
    assert!(!reminder_due(T0, None, 1, T0 + HOUR_MS - 1));
    assert!(reminder_due(T0, None, 1, T0 + HOUR_MS));
    assert!(!reminder_due(T0, Some(T0 + HOUR_MS), 1, T0 + HOUR_MS + 1));
    // A reminder for an older question does not count.
    assert!(reminder_due(T0, Some(T0 - 5), 1, T0 + HOUR_MS));
    assert!(default_due(T0, 2, T0 + 2 * HOUR_MS));
    assert!(!default_due(T0, 0, T0 + 99 * HOUR_MS));
}

#[tokio::test]
async fn a_question_waiting_three_and_a_half_hours_gets_three_hourly_reminders() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    {
        let mut s = app.settings.write().unwrap();
        s.auto_answer = true;
        s.wait_remind_every_hours = 1;
    }
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    app.store.save_task(&task).unwrap();
    let repo_path = task.repo.clone();
    for step in 1..=7 {
        app.wait_sla_tick(T0 + step * HALF_HOUR).await;
        let due = [2, 4, 6].iter().filter(|s| **s <= step).count();
        calls_reach(&repo, due).await;
        crate::engine::chat::test_support::wait_idle(&app, &repo_path).await;
    }
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        3
    );
    // A tick right after a restart gives no burst for the missed hours.
    app.wait_sla_tick(T0 + 7 * HALF_HOUR + 1000).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        3
    );
}

#[tokio::test]
async fn a_question_is_held_while_the_overseer_turn_runs_and_released_when_it_escalates() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    let owner_repo = dir.path().join("owner");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&owner_repo).unwrap();
    let plain = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    let owner = waiting_in(&owner_repo, QuestionKind::Permission, &["Allow once"]);
    app.store.save_task(&plain).unwrap();
    app.store.save_task(&owner).unwrap();
    let mut rx = app.events_tx.subscribe();
    app.wake_overseer_for_waiting_after_restart();
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &repo.to_string_lossy()).await;
    // The fake overseer answers nothing: the turn ended with the question open.
    let flags = held_flags(&mut rx, &plain.id);
    assert_eq!(flags, vec![true, false], "held, then released");
    let after = app.store.load_task(&plain.id).unwrap().unwrap();
    assert!(!after.question.clone().unwrap().held_for_overseer);
    assert!(
        held_flags(&mut rx, &owner.id).is_empty(),
        "owner-only never changes"
    );
    let owner = app.store.load_task(&owner.id).unwrap().unwrap();
    assert!(!owner.question.clone().unwrap().held_for_overseer);
}
