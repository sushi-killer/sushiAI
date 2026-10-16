use crate::clock::now_ms;
use crate::engine::owner_notice;
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::state::App;
use crate::engine::test_support::test_app;
use crate::engine::wait_sla::tests::support::boundary_note;
use crate::engine::wait_sla::tests::support::calls_reach;
use crate::engine::wait_sla::tests::support::fresh_app;
use crate::engine::wait_sla::tests::support::kinds;
use crate::engine::wait_sla::tests::support::park_in_quiet;
use crate::engine::wait_sla::tests::support::told;
use crate::engine::wait_sla::tests::support::waiting_in;
use crate::engine::wait_sla::tests::support::window_around;
use crate::engine::wait_sla::tests::support::HALF_HOUR;
use crate::engine::wait_sla::tests::support::T0;
use crate::policy::waits::HOUR_MS;
use crate::task::QuestionKind;
use crate::task::TaskStatus;

#[tokio::test]
async fn a_digest_restarts_the_reminder_window_and_the_reminder_comes_once_after_it() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    {
        let mut s = app.settings.write().unwrap();
        s.auto_answer = true;
        s.wait_remind_every_hours = 1;
    }
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    app.store.save_task(&task).unwrap();
    // The owner heard the question 30 minutes after it was asked.
    let key = owner_notice::decision_key(&task.id, task.question.as_ref().unwrap());
    let mut digest = crate::task::notes::overseer::Entry::new(
        "digest",
        &repo_str,
        None,
        "Digest",
        crate::clock::now_ms(),
    )
    .with_decision(&[key]);
    digest.ts = T0 + HALF_HOUR;
    app.record_overseer(digest);
    // One interval after the question, the notice is only 30 minutes old: no wake.
    app.wait_sla_tick(T0 + HOUR_MS).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        0
    );
    // One interval after the notice: one reminder, and none again at once.
    app.wait_sla_tick(T0 + HALF_HOUR + HOUR_MS).await;
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &repo_str).await;
    app.wait_sla_tick(T0 + HALF_HOUR + HOUR_MS + 1000).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        1
    );
}

#[tokio::test]
async fn quiet_hours_start_no_reminder_and_the_held_notice_is_posted_once_after_them() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    {
        let mut s = app.settings.write().unwrap();
        s.auto_answer = true;
        s.wait_remind_every_hours = 1;
        // A window that holds the local minute of the tick, in any zone.
        let m = owner_notice::minute_of_day(T0 + 2 * HOUR_MS);
        let (from, to) = (m, (m + 120) % 1440);
        s.quiet_hours = format!(
            "{:02}:{:02}-{:02}:{:02}",
            from / 60,
            from % 60,
            to / 60,
            to % 60
        );
    }
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    app.store.save_task(&task).unwrap();
    let key = owner_notice::decision_key(&task.id, task.question.as_ref().unwrap());
    app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        0,
        "no reminder wake in quiet hours"
    );
    // A notice held overnight, with the digest off, goes out once the window ends.
    let mut held = crate::task::notes::overseer::Entry::new(
        "skipped",
        &repo_str,
        Some(&task.id),
        owner_notice::DEFERRED,
        crate::clock::now_ms(),
    )
    .with_detail("Which way?\nA or B\nI suggest A.")
    .with_decision(&[key]);
    held.ts = T0;
    app.record_overseer(held);
    app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
    assert!(!crate::engine::chat::test_support::thread_has_text(
        &app,
        &repo_str,
        "Which way?\nA or B\nI suggest A."
    ));
    app.settings.write().unwrap().quiet_hours = String::new();
    app.wait_sla_tick(T0 + 3 * HOUR_MS).await;
    assert!(crate::engine::chat::test_support::thread_has_text(
        &app,
        &repo_str,
        "Which way?\nA or B\nI suggest A."
    ));
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
    app.wait_sla_tick(T0 + 3 * HOUR_MS + 1000).await;
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1, "once");
}

#[tokio::test]
async fn quiet_hours_owner_only_question_is_noticed_once_at_the_boundary() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let first = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
    let second = park_in_quiet(&app, &repo, &["Allow always", "Deny"]);
    let note = boundary_note(&[&first, &second]);
    let later = now_ms() + HOUR_MS;
    // The window is still active: nothing is posted.
    app.settings.write().unwrap().quiet_hours = window_around(later);
    app.wait_sla_tick(later).await;
    assert!(!crate::engine::chat::test_support::thread_has_text(
        &app, &repo_str, &note
    ));
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 0);
    // The window ends: one note lists both, and a second tick adds none.
    app.settings.write().unwrap().quiet_hours = String::new();
    app.wait_sla_tick(later + 1000).await;
    assert_eq!(
        crate::engine::chat::test_support::thread_text_count(&app, &repo_str, &note),
        1
    );
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
    app.wait_sla_tick(later + 2000).await;
    assert_eq!(
        crate::engine::chat::test_support::thread_text_count(&app, &repo_str, &note),
        1
    );
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
}

#[tokio::test]
async fn a_digest_that_told_the_question_suppresses_the_boundary_notice() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    for digest_hour in [None, Some(7)] {
        let (app, dir) = test_app();
        app.settings.write().unwrap().digest_hour = digest_hour;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        let told_task = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
        let open_task = park_in_quiet(&app, &repo, &["Allow always", "Deny"]);
        let later = now_ms() + HOUR_MS;
        // A digest that ran after the first parked already told the owner.
        told(&app, &told_task, now_ms() + 1);
        app.settings.write().unwrap().quiet_hours = String::new();
        app.wait_sla_tick(later).await;
        assert!(crate::engine::chat::test_support::thread_has_text(
            &app,
            &repo_str,
            &boundary_note(&[&open_task])
        ));
        assert!(!crate::engine::chat::test_support::thread_has_text(
            &app,
            &repo_str,
            &boundary_note(&[&told_task, &open_task])
        ));
    }
}

#[tokio::test]
async fn the_boundary_notice_survives_a_restart_and_a_full_overseer_log() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let task = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
    let note = boundary_note(&[&task]);
    // Appends past the cap in one write; reads keep the newest `MAX_LINES`.
    let flood = |app: &App| {
        use std::io::Write;
        let mut lines = String::new();
        for i in 0..crate::store::overseer::MAX_LINES + 10 {
            let entry = crate::task::notes::overseer::Entry::new(
                "skipped",
                "/other",
                None,
                &format!("n{i}"),
                crate::clock::now_ms(),
            );
            lines.push_str(&serde_json::to_string(&entry).unwrap());
            lines.push('\n');
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(app.data_dir.join("overseer.jsonl"))
            .unwrap();
        file.write_all(lines.as_bytes()).unwrap();
    };
    // A restart and a full log lose nothing before the notice.
    let again = fresh_app(dir.path());
    flood(&again);
    let later = now_ms() + HOUR_MS;
    again.wait_sla_tick(later).await;
    assert_eq!(
        crate::engine::chat::test_support::thread_text_count(&again, &repo_str, &note),
        1
    );
    // The notice entry is evicted and the daemon restarts: still once.
    flood(&again);
    let third = fresh_app(dir.path());
    third.wait_sla_tick(later + 1000).await;
    assert_eq!(
        crate::engine::chat::test_support::thread_text_count(&third, &repo_str, &note),
        1
    );
}

#[tokio::test]
async fn an_answered_owner_only_question_is_not_noticed_at_the_boundary() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let mut answered = park_in_quiet(&app, &repo, &["Allow once", "Deny"]);
    let mut archived = park_in_quiet(&app, &repo, &["Allow always", "Deny"]);
    let kept = park_in_quiet(&app, &repo, &["Allow here", "Deny"]);
    answered.fixture_data().status = TaskStatus::Running;
    answered.fixture_data().question = None;
    app.store.save_task(&answered).unwrap();
    archived.fixture_data().archived = true;
    app.store.save_task(&archived).unwrap();
    app.settings.write().unwrap().quiet_hours = String::new();
    // Parked outside quiet hours: nothing is recorded for it.
    let outside = waiting_in(&repo, QuestionKind::Permission, &["Allow outside", "Deny"]);
    app.store.save_task(&outside).unwrap();
    wake_orchestrator_for_waiting(&app, &outside);
    app.wait_sla_tick(now_ms() + HOUR_MS).await;
    assert!(crate::engine::chat::test_support::thread_has_text(
        &app,
        &repo_str,
        &boundary_note(&[&kept])
    ));
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
}
