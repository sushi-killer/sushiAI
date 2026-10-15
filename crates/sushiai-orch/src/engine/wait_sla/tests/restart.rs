use crate::clock::now_ms;
use crate::engine::owner_notice;
use crate::engine::state::App;
use crate::engine::test_support::test_app;
use crate::engine::wait_sla::tests::support::calls_reach;
use crate::engine::wait_sla::tests::support::kinds;
use crate::engine::wait_sla::tests::support::told;
use crate::engine::wait_sla::tests::support::waiting_in;
use crate::task::QuestionKind;
use crate::task::TaskStatus;

#[tokio::test]
async fn a_restart_wakes_the_overseer_once_per_repo_for_non_owner_only_waits() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    let owner_repo = dir.path().join("owner");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&owner_repo).unwrap();
    for task in [
        waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]),
        waiting_in(&repo, QuestionKind::PlanQuestion, &["A"]),
        waiting_in(&owner_repo, QuestionKind::Permission, &["Allow once"]),
    ] {
        app.store.save_task(&task).unwrap();
    }
    app.wake_overseer_for_waiting_after_restart();
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &repo.to_string_lossy()).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        1
    );
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&owner_repo),
        0
    );
}

#[tokio::test]
async fn a_restart_tells_the_owner_once_and_a_second_restart_says_nothing() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    app.settings.write().unwrap().wait_remind_every_hours = 4;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    task.fixture_data().question.as_mut().unwrap().asked_at = Some(now_ms());
    app.store.save_task(&task).unwrap();
    // The fake overseer answers nothing, so the question is still open: it is told.
    app.wake_overseer_for_waiting_after_restart();
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &repo_str).await;
    assert!(crate::engine::chat::test_support::thread_has_text(
        &app,
        &repo_str,
        "Overseer note."
    ));
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 1);
    // A new daemon reads the same overseer log and tells nothing again.
    let again = App::new(
        dir.path().join("data"),
        dir.path().join("home"),
        "orchd".to_string(),
    )
    .unwrap();
    again.assume_harnesses_installed();
    again.settings.write().unwrap().auto_answer = true;
    again.settings.write().unwrap().quiet_hours = String::new();
    again.wake_overseer_for_waiting_after_restart();
    calls_reach(&repo, 2).await;
    crate::engine::chat::test_support::wait_idle(&again, &repo_str).await;
    let notes =
        crate::engine::chat::test_support::thread_text_count(&again, &repo_str, "Overseer note.");
    assert_eq!(notes, 1, "no second message");
    assert_eq!(kinds(&again, &repo_str, "asked_owner"), 1);
    assert_eq!(kinds(&again, &repo_str, "skipped"), 1);
    let skipped = crate::store::overseer::read(&again.data_dir, Some(&repo_str), Some(1));
    assert_eq!(skipped[0].text, owner_notice::DUPLICATE);
}

#[tokio::test]
async fn a_restart_with_only_told_questions_posts_nothing_and_logs_no_asked_owner() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    app.settings.write().unwrap().wait_remind_every_hours = 4;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    app.store.save_task(&task).unwrap();
    told(&app, &task, now_ms() - 1000);
    let before = kinds(&app, &repo_str, "asked_owner");
    app.wake_overseer_for_waiting_after_restart();
    calls_reach(&repo, 1).await;
    crate::engine::chat::test_support::wait_idle(&app, &repo_str).await;
    assert!(!crate::engine::chat::test_support::thread_has_text(
        &app,
        &repo_str,
        "Overseer note."
    ));
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), before);
}

#[tokio::test]
async fn a_turn_whose_question_is_gone_is_done_and_never_reaches_the_chat() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    // The task is not waiting any more when the turn ends: it settled the question.
    let mut task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A"]);
    let id = task.id.clone();
    app.store.save_task(&task).unwrap();
    assert!(crate::engine::chat::wake::wake_overseer(
        &app,
        &repo_str,
        crate::engine::chat::wake::WakeRequest::new("p".into(), false, "waiting", Some(&id))
    ));
    task.resume_phase(TaskStatus::Running, task.updated_at);
    task.clear_question();
    app.store.save_task(&task).unwrap();
    crate::engine::chat::test_support::wait_idle(&app, &repo_str).await;
    assert!(!crate::engine::chat::test_support::thread_has_text(
        &app,
        &repo_str,
        "Overseer note."
    ));
    assert_eq!(kinds(&app, &repo_str, "asked_owner"), 0);
    assert_eq!(kinds(&app, &repo_str, "done"), 1);
}
