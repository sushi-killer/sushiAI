use crate::engine::decision::fail_and_continue;
use crate::engine::decision::LoopSignal;
use crate::engine::decision::EXHAUSTED_ANSWER_PREFIX;
use crate::engine::ladder;
use crate::engine::questions::answers::non_owner_refusal;
use crate::engine::questions::answers::overseer_allowed_answers;
use crate::engine::questions::parking::park_for_answer;
use crate::engine::questions::wake::wake_orchestrator_for_done;
use crate::engine::run_control::TaskControl;
use crate::engine::state::App;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::task::AnsweredBy;
use crate::task::AskedBy;
use crate::task::FailureKind;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Risk;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::HOLD_TTL_MS;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

/// A waiting task in a real repo directory with the fake harness, and
/// the number of overseer turns that ran for it.
async fn waiting_wakes(auto_answer: bool, kind: QuestionKind) -> usize {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = auto_answer;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    task.fixture_data().question = Some(Question::new(
        "Which way?",
        vec!["A".into(), "B".into()],
        kind,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    app.store.save_task(&task).unwrap();
    let cancel = CancelToken::new();
    let pending = Arc::new(StdMutex::new(None));
    let _id = task.id.clone();
    let parked = {
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        tokio::spawn(async move {
            park_for_answer(&app, &mut task, &pending, &cancel, &mut None).await;
        })
    };
    while pending.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancel.cancel();
    parked.await.unwrap();
    crate::engine::chat::test_support::wait_idle(&app, repo.to_str().unwrap()).await;
    crate::engine::chat::test_support::fake_overseer_calls(&repo)
}

#[tokio::test]
async fn a_parked_task_wakes_the_overseer_once_only_with_auto_answer_on() {
    assert_eq!(waiting_wakes(true, QuestionKind::AgentQuestion).await, 1);
    assert_eq!(waiting_wakes(false, QuestionKind::AgentQuestion).await, 0);
    assert_eq!(waiting_wakes(true, QuestionKind::Budget).await, 0);
}

#[tokio::test]
async fn only_the_done_transition_wakes_the_overseer_once() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    app.store.save_task(&task).unwrap();
    // A rebroadcast of an old done task wakes nothing.
    app.broadcast_task(&task);
    crate::engine::chat::test_support::wait_idle(&app, task.repo.as_str()).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        0
    );
    wake_orchestrator_for_done(&app, &mut task);
    crate::engine::chat::test_support::wait_idle(&app, task.repo.as_str()).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        1
    );
    assert!(
        app.store
            .load_task(&task.id)
            .unwrap()
            .unwrap()
            .overseer_woken
    );
    // Again (a restart, an unarchive) and a subtask: still one.
    task.fixture_data().archived = false;
    wake_orchestrator_for_done(&app, &mut task);
    let mut part = task_with_status(TaskStatus::Done);
    part.fixture_data().repo = task.repo.clone();
    part.fixture_data().parent = Some(task.id.clone());
    wake_orchestrator_for_done(&app, &mut part);
    let mut landed = task_with_status(TaskStatus::Done);
    landed.fixture_data().repo = task.repo.clone();
    landed.fixture_data().landed_sha = Some("abc".into());
    wake_orchestrator_for_done(&app, &mut landed);
    crate::engine::chat::test_support::wait_idle(&app, task.repo.as_str()).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        1
    );
}

fn exhausted_task() -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().question = Some(Question::new(
        "Attempts keep failing with a review failure: continue, change approach, or stop?",
        vec!["continue".into(), "stop".into()],
        QuestionKind::AttemptsFailing,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    task
}

#[tokio::test]
async fn an_agent_sourced_answer_is_an_orchestrator_line_and_never_the_owners() {
    let (app, _dir) = test_app();
    let mut task = exhausted_task();
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let parked = {
        let (app, pending, cancel, _id) =
            (app.clone(), pending.clone(), cancel.clone(), id.clone());
        tokio::spawn(async move {
            let answered = park_for_answer(&app, &mut task, &pending, &cancel, &mut None).await;
            (answered, task)
        })
    };
    let tx = loop {
        if let Some(tx) = pending.lock().unwrap().take() {
            break tx;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    };
    app.agent_answers
        .lock()
        .unwrap()
        .insert(id.clone(), AnsweredBy::Orchestrator);
    tx.send("continue".into()).unwrap();
    let (answered, task) = parked.await.unwrap();
    assert_eq!(answered.as_deref(), Some("continue"));
    assert_eq!(app.take_agent_answer(&id), Some(AnsweredBy::Orchestrator));
    assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
    assert_eq!(
        task.decisions
            .iter()
            .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
            .count(),
        1
    );
}

#[test]
fn the_overseer_is_only_offered_answers_the_gates_let_it_give() {
    let settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    let permission = Question::new(
        "Allow the write?",
        vec![
            crate::policy::permissions::ALLOW_ONCE.into(),
            crate::policy::permissions::ALLOW_ALWAYS.into(),
            crate::policy::permissions::DENY.into(),
        ],
        QuestionKind::Permission,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    task.fixture_data().question = Some(permission.clone());
    assert_eq!(
        overseer_allowed_answers(&settings, &task, &permission),
        vec![crate::policy::permissions::DENY.to_string()]
    );
    let deploy = Question::new(
        "Deploy to production now?",
        vec!["yes".into(), "no".into()],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    assert!(overseer_allowed_answers(&settings, &task, &deploy).is_empty());
    let mut tagged = Question::new(
        "Which approach?",
        vec!["A".into(), "B".into()],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    tagged.risk = vec![Risk::Security];
    assert!(overseer_allowed_answers(&settings, &task, &tagged).is_empty());
    let plain = Question::new(
        "Which way?",
        vec!["A".into(), "B".into()],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    assert_eq!(
        overseer_allowed_answers(&settings, &task, &plain),
        vec!["A".to_string(), "B".to_string()]
    );
}

#[tokio::test]
async fn the_overseer_wake_carries_the_typed_question_and_its_allowed_answers() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    task.fixture_data().question = Some(Question::new(
        "Which way?",
        vec!["A".into(), "B".into()],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    app.store.save_task(&task).unwrap();
    let cancel = CancelToken::new();
    let pending = Arc::new(StdMutex::new(None));
    let _id = task.id.clone();
    let parked = {
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        tokio::spawn(async move {
            park_for_answer(&app, &mut task, &pending, &cancel, &mut None).await;
        })
    };
    while pending.lock().unwrap().is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancel.cancel();
    parked.await.unwrap();
    crate::engine::chat::test_support::wait_idle(&app, repo.to_str().unwrap()).await;
    let calls = std::fs::read_to_string(repo.join(".overseer-calls")).unwrap();
    assert!(
        calls.contains(r#""allowedAnswers":["A","B"]"#),
        "the overseer must be handed its allowed answers: {calls}"
    );
    assert!(calls.contains(r#""kind":"agent_question""#), "{calls}");
}

#[tokio::test]
async fn an_exhausted_failure_goes_to_the_overseer_with_no_extra_turn() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    task.fixture_data().worktree = worktree.to_string_lossy().to_string();
    task.fixture_data()
        .attempts
        .push(crate::engine::test_support::attempt_with_failure(
            1, "verify",
        ));
    let id = task.id.clone();
    let mut budget = 1;
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let waiter = {
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        tokio::spawn(async move {
            let signal = fail_and_continue(
                &app,
                &id,
                &mut task,
                0,
                FailureKind::Verify,
                "npm test failed".into(),
                &mut budget,
                &pending,
                &cancel,
                &mut None,
            )
            .await;
            (matches!(signal, LoopSignal::Stop), task)
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while pending.lock().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the failure never parked for the overseer");
    // Parked: the only agent run was the overseer's, in the repository.
    assert!(!worktree.join(".overseer-calls").exists());
    // Woken once for the question; the turn is left to end on its own.
    for _ in 0..400 {
        if crate::engine::chat::test_support::fake_overseer_calls(&repo) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(&repo),
        1
    );
    cancel.cancel();
    let (stopped, _) = waiter.await.unwrap();
    assert!(stopped);
}

#[test]
fn the_overseer_may_only_continue_or_change_approach_on_attempts_failing() {
    let settings = Settings::default();
    let task = task_with_status(TaskStatus::Waiting);
    let question = Question::new(
        "Attempts keep failing with a verify failure: continue, change approach, or stop?",
        vec!["continue".into(), "change approach".into(), "stop".into()],
        QuestionKind::AttemptsFailing,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    assert_eq!(
        overseer_allowed_answers(&settings, &task, &question),
        vec!["continue".to_string(), "change approach".to_string()]
    );
    for answer in ["ask the owner", "stop"] {
        assert!(
            non_owner_refusal(&settings, &task, &question, answer, AnsweredBy::Overseer).is_some(),
            "{answer}"
        );
    }
    assert!(non_owner_refusal(
        &settings,
        &task,
        &question,
        "continue - create fixed.txt",
        AnsweredBy::Overseer
    )
    .is_none());
}

#[tokio::test]
async fn an_exhausted_question_the_overseer_leaves_open_ends_the_ladder() {
    let _harness = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    task.fixture_data().worktree = worktree.to_string_lossy().to_string();
    task.fixture_data()
        .attempts
        .push(crate::engine::test_support::attempt_with_failure(
            1, "verify",
        ));
    task.fixture_data().loop_state.approach_changed_after = Some(1);
    let id = task.id.clone();
    let mut budget = 1;
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    let waiter = {
        let (app, pending, cancel, id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        tokio::spawn(async move {
            let signal = fail_and_continue(
                &app,
                &id,
                &mut task,
                0,
                FailureKind::Verify,
                "npm test failed".into(),
                &mut budget,
                &pending,
                &cancel,
                &mut None,
            )
            .await;
            (matches!(signal, LoopSignal::Stop), task)
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while pending.lock().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the failure never parked for the overseer");
    // The overseer's turn ends without an answer.
    crate::engine::chat::test_support::wait_idle(&app, repo.to_str().unwrap()).await;
    let (stopped, _) = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .expect("the ladder never ended the parked loop")
        .unwrap();
    assert!(stopped);
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task.question.is_none());
    assert!(task
        .decisions
        .iter()
        .any(|d| d.starts_with("Ladder: stopped")));
}

#[tokio::test]
async fn an_exhausted_failure_never_waits_when_no_overseer_turn_starts() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let mut task = task_with_status(TaskStatus::Running);
    // The repository is not a directory: the wake does not start.
    task.fixture_data().repo = dir.path().join("missing").to_string_lossy().to_string();
    task.fixture_data().worktree = worktree.to_string_lossy().to_string();
    task.fixture_data()
        .attempts
        .push(crate::engine::test_support::attempt_with_failure(
            1, "verify",
        ));
    let id = task.id.clone();
    app.store.save_task(&task).unwrap();
    let mut budget = 1;
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let (app2, pending2, cancel2) = (app.clone(), pending.clone(), cancel.clone());
    let waiter = tokio::spawn(async move {
        fail_and_continue(
            &app2,
            &id,
            &mut task,
            0,
            FailureKind::Verify,
            "npm test failed".into(),
            &mut budget,
            &pending2,
            &cancel2,
            &mut None,
        )
        .await;
        task
    });
    let task = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .expect("the failure waited for an overseer that never started")
        .unwrap();
    assert_ne!(task.status, TaskStatus::Waiting);
    assert!(pending.lock().unwrap().is_none());
}

fn held_exhausted(app: &App, approach_changed: bool) -> Task {
    let mut task = exhausted_task();
    task.fixture_data().attempts = vec![crate::engine::test_support::attempt_with_failure(
        1, "verify",
    )];
    if approach_changed {
        task.fixture_data().loop_state.approach_changed_after = Some(1);
    }
    if let Some(q) = task.fixture_data().question.as_mut() {
        q.held_for_overseer = true;
        q.held_at = Some(1_000);
    }
    app.store.save_task(&task).unwrap();
    task
}

#[tokio::test]
async fn a_hold_that_expires_on_an_exhausted_question_takes_the_next_ladder_rung() {
    let (app, _dir) = test_app();
    let task = held_exhausted(&app, true);
    app.release_expired_holds(1_000 + HOLD_TTL_MS + 1);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Failed);
    assert!(saved.question.is_none());

    // Before the changed approach, the rung is the change, not the end.
    let (app, _dir) = test_app();
    let task = held_exhausted(&app, false);
    app.release_expired_holds(1_000 + HOLD_TTL_MS + 1);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(saved.question.is_none());
    assert!(ladder::approach_changed(&saved));
}

#[tokio::test]
async fn a_hold_that_expires_while_an_overseer_wake_is_queued_keeps_the_question() {
    let (app, _dir) = test_app();
    let task = held_exhausted(&app, true);
    app.deferred_wakes.lock().unwrap().insert(
        task.repo.clone(),
        vec![crate::engine::chat::wake::test_deferred_wake(
            "waiting", &task.id,
        )],
    );
    app.release_expired_holds(1_000 + HOLD_TTL_MS + 1);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Waiting);
    let q = saved.question.clone().expect("the question stays open");
    assert!(!q.held_for_overseer);
}

#[tokio::test]
async fn a_restart_keeps_a_held_exhausted_question_for_the_restart_wake() {
    let (app, _dir) = test_app();
    let task = held_exhausted(&app, false);
    app.release_stale_holds();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Waiting);
    assert!(!saved.question.clone().unwrap().held_for_overseer);
    // With nobody to ask again, the start takes the changed approach.
    app.retire_ladder_questions();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(ladder::approach_changed(&saved));
}

#[test]
fn the_answer_policy_stays_under_the_hard_limits_like_the_overseer() {
    let settings = Settings::default();
    let task = task_with_status(TaskStatus::Waiting);
    let mut question = Question::new(
        "How should the old data go?",
        vec!["delete the old data".into(), "keep the old data".into()],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    question.risk = vec![Risk::Destructive];
    for caller in [AnsweredBy::Policy, AnsweredBy::Overseer] {
        assert!(
            non_owner_refusal(&settings, &task, &question, "keep the old data", caller).is_some(),
            "{caller:?}"
        );
    }
    assert!(non_owner_refusal(
        &settings,
        &task,
        &question,
        "keep the old data",
        AnsweredBy::Lead
    )
    .is_none());
}

#[tokio::test]
async fn an_agent_stop_is_an_orchestrator_line_and_pr_refuses_a_branch() {
    let (app, _dir) = test_app();
    let task = exhausted_task();
    app.store.save_task(&task).unwrap();
    let stop = json!({"id": task.id, "answer": "stop", "source": "chat"});
    app.handle_task_answer(stop).await.unwrap();
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
    assert!(task
        .decisions
        .iter()
        .any(|d| d.starts_with("Orchestrator:")));
    let err = app
        .handle_task_pr(json!({"id": task.id, "branch": "main", "source": "chat"}))
        .await
        .unwrap_err();
    assert!(err.contains("own branch"), "{err}");
}
