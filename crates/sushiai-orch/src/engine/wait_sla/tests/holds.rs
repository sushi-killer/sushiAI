use crate::clock::now_ms;
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::test_support::test_app;
use crate::engine::wait_sla::tests::support::calls_reach;
use crate::engine::wait_sla::tests::support::held;
use crate::engine::wait_sla::tests::support::held_flags;
use crate::engine::wait_sla::tests::support::waiting_in;
use crate::engine::wait_sla::tests::support::T0;
use crate::harness::cancel::CancelToken;
use crate::task::AskedBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::HOLD_TTL_MS;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
async fn a_queued_wake_holds_the_question_at_once_and_a_wake_that_cannot_run_never_does() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    app.store.save_task(&task).unwrap();
    let repo_str = task.repo.clone();
    // The chat is busy: the wake is queued and already the overseer's,
    // so the owner gets no notice for it.
    app.chat_turns
        .lock()
        .unwrap()
        .insert(repo_str.clone(), CancelToken::new());
    wake_orchestrator_for_waiting(&app, &task);
    assert!(held(&app, &task.id), "a queued wake holds the question");
    // The busy turn ends: the queued wake runs, holds, then releases.
    let mut rx = app.events_tx.subscribe();
    app.chat_turns.lock().unwrap().remove(&repo_str);
    crate::engine::chat::wake::fire_deferred_wakes(&app, &repo_str);
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &repo_str).await;
    assert_eq!(held_flags(&mut rx, &task.id), vec![true, false]);
    assert!(!held(&app, &task.id));
    // A daemon shutting down cannot wake it: nothing is held.
    app.shutting_down.store(true, Ordering::SeqCst);
    wake_orchestrator_for_waiting(&app, &task);
    assert!(!held(&app, &task.id));
}

#[tokio::test]
async fn turning_auto_answer_on_wakes_the_overseer_for_a_question_parked_while_it_was_off() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    app.store.save_task(&task).unwrap();
    let mut settings = app.settings.read().unwrap().clone();
    assert!(!settings.auto_answer);
    wake_orchestrator_for_waiting(&app, &task);
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        0,
        "off: no wake"
    );

    settings.auto_answer = true;
    app.handle_settings_set(json!({ "settings": settings.clone() }))
        .await
        .unwrap();
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &task.repo).await;
    // Saving settings that were already answering wakes nobody again.
    app.handle_settings_set(json!({ "settings": settings }))
        .await
        .unwrap();
    crate::engine::chat::test_support::wait_idle(&app, &task.repo).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        1
    );
}

#[tokio::test]
async fn a_hold_expires_after_ten_minutes_and_the_tick_releases_it() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    let q = task.fixture_data().question.as_mut().unwrap();
    q.held_for_overseer = true;
    q.held_at = Some(T0);
    assert!(q.held_now(T0 + HOLD_TTL_MS - 1));
    assert!(!q.held_now(T0 + HOLD_TTL_MS));
    app.store.save_task(&task).unwrap();
    let mut rx = app.events_tx.subscribe();
    app.wait_sla_tick(T0 + 1000).await;
    // Fresh in the test's eyes? The tick compares with the clock it is given.
    assert!(held(&app, &task.id), "still within ten minutes");
    app.wait_sla_tick(T0 + HOLD_TTL_MS + 1000).await;
    assert!(!held(&app, &task.id));
    assert_eq!(held_flags(&mut rx, &task.id), vec![false]);
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .question
        .clone()
        .unwrap()
        .held_at
        .is_none());
}

#[tokio::test]
async fn a_hung_overseer_turn_is_cancelled_logged_as_timed_out_and_releases_its_hold() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    {
        let mut s = app.settings.write().unwrap();
        s.auto_answer = true;
    }
    app.overseer_turn_timeout_ms
        .store(400, std::sync::atomic::Ordering::SeqCst);
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    // The fake harness sleeps 30 s when this file exists.
    std::fs::write(repo.join(".overseer-slow"), "").unwrap();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    app.store.save_task(&task).unwrap();
    let repo_str = task.repo.clone();
    let started = std::time::Instant::now();
    wake_orchestrator_for_waiting(&app, &task);
    assert!(held(&app, &task.id), "held while the turn runs");
    crate::engine::chat::test_support::wait_idle(&app, &repo_str).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!held(&app, &task.id), "released when the turn timed out");
    let log = crate::overseer::read(&app.data_dir, Some(&repo_str), None);
    assert!(
        log.iter()
            .any(|e| e.kind == "failed" && e.text.contains("timed out")),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_turn_releases_only_its_own_hold_and_not_when_another_wake_is_queued() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    let repo_str = task.repo.clone();
    let q = task.fixture_data().question.as_mut().unwrap();
    q.held_for_overseer = true;
    q.held_at = Some(2000);
    app.store.save_task(&task).unwrap();
    // An older turn's hold is not this question's current hold.
    app.release_holds(&repo_str, &[(task.id.clone(), 1000)], false);
    assert!(held(&app, &task.id));
    // A queued wake for the task will hold it again: no flicker.
    app.deferred_wakes.lock().unwrap().insert(
        repo_str.clone(),
        vec![crate::engine::chat::wake::test_deferred_wake(
            "waiting", &task.id,
        )],
    );
    app.release_holds(&repo_str, &[(task.id.clone(), 2000)], false);
    assert!(held(&app, &task.id));
    app.deferred_wakes.lock().unwrap().clear();
    app.release_holds(&repo_str, &[(task.id.clone(), 2000)], false);
    assert!(!held(&app, &task.id));
}

#[tokio::test]
async fn holds_left_by_a_stopped_daemon_are_released_on_start() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    let q = task.fixture_data().question.as_mut().unwrap();
    q.held_for_overseer = true;
    q.held_at = Some(now_ms());
    app.store.save_task(&task).unwrap();
    app.release_stale_holds();
    assert!(!held(&app, &task.id));
}

#[test]
fn the_flag_is_omitted_when_false_and_camel_case_when_true() {
    let mut q = Question::new(
        "Which?",
        vec![],
        QuestionKind::AgentQuestion,
        AskedBy::Plan,
        crate::clock::now_ms(),
    );
    assert!(serde_json::to_value(&q)
        .unwrap()
        .get("heldForOverseer")
        .is_none());
    q.held_for_overseer = true;
    assert_eq!(serde_json::to_value(&q).unwrap()["heldForOverseer"], true);
}
