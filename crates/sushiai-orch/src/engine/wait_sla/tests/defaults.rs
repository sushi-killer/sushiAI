use crate::clock::now_ms;
use crate::engine::planning::questions::ask_plan_question;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskControl;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::wait_sla::tests::support::waiting_in;
use crate::engine::wait_sla::tests::support::T0;
use crate::harness::cancel::CancelToken;
use crate::policy::waits::HOUR_MS;
use crate::task::AnsweredBy;
use crate::task::QuestionKind;
use crate::task::TaskStatus;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

#[tokio::test]
async fn the_default_answers_a_waiting_question_with_its_cautious_option_as_an_assumption() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    app.settings.write().unwrap().wait_default_after_hours = 2;
    let mut task = waiting_in(
        &repo,
        QuestionKind::PlanQuestion,
        &["rewrite it", "keep the old API (cautious)"],
    );
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let waiter = {
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        tokio::spawn(async move {
            let _id = task.id.clone();
            let answer = QuestionSlot::new(
                &crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .answer(&app, &mut task)
            .await;
            (answer, task)
        })
    };
    // The parked loop's control, as `spawn_task_loop` registers it.
    for _ in 0..200 {
        if pending.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    app.wait_sla_tick(T0 + HOUR_MS).await;
    assert!(pending.lock().unwrap().is_some(), "not yet due");
    app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
    let (answer, task) = waiter.await.unwrap();
    assert_eq!(answer.as_deref(), Ok("keep the old API (cautious)"));
    assert_eq!(task.assumptions.len(), 1);
    let a = &task.assumptions[0];
    assert_eq!(
        (a.by.as_str(), a.kind),
        ("policy", Some(QuestionKind::PlanQuestion))
    );
    assert!(a.evidence.contains("2 h"), "{}", a.evidence);
    assert!(task.decisions.iter().any(|d| d.starts_with("Policy:")));
    assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
}

#[tokio::test]
async fn the_default_option_the_agent_named_is_taken_without_a_cautious_word() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    app.settings.write().unwrap().wait_default_after_hours = 2;
    let mut task = waiting_in(
        &repo,
        QuestionKind::AgentQuestion,
        &["rewrite it", "keep the old API"],
    );
    task.fixture_data()
        .question
        .as_mut()
        .unwrap()
        .default_option = Some(1);
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let waiter = {
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        tokio::spawn(async move {
            let _id = task.id.clone();
            let answer = QuestionSlot::new(
                &crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .answer(&app, &mut task)
            .await;
            (answer, task)
        })
    };
    for _ in 0..200 {
        if pending.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    app.wait_sla_tick(T0 + 2 * HOUR_MS).await;
    let (answer, task) = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .expect("the default option never answered the question")
        .unwrap();
    assert_eq!(answer.as_deref(), Ok("keep the old API"));
    assert_eq!(task.assumptions.len(), 1);
    assert_eq!(task.assumptions[0].by, "policy");
}

#[tokio::test]
async fn the_default_never_answers_an_owner_only_kind_or_a_question_without_a_cautious_option() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    app.settings.write().unwrap().wait_default_after_hours = 1;
    let owner = waiting_in(&repo, QuestionKind::Budget, &["raise (cautious)", "stop"]);
    let plain = waiting_in(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    app.store.save_task(&owner).unwrap();
    app.store.save_task(&plain).unwrap();
    app.wait_sla_tick(T0 + 50 * HOUR_MS).await;
    for t in [&owner, &plain] {
        let t = app.store.load_task(&t.id).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Waiting);
        assert!(t.question.is_some() && t.assumptions.is_empty());
    }
}

#[tokio::test]
async fn a_default_answer_to_a_planner_question_is_a_policy_assumption_and_leaves_nothing_behind() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    app.settings.write().unwrap().wait_default_after_hours = 2;
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let waiter = {
        let (app, pending, cancel, id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        tokio::spawn(async move {
            ask_plan_question(
                &app,
                &id,
                crate::task::Question {
                    risk: Vec::new(),
                    default_option: None,
                    ..crate::task::Question::new(
                        "Which way?",
                        vec!["rewrite".into(), "keep it (reversible)".into()],
                        crate::task::QuestionKind::PlanQuestion,
                        crate::task::AskedBy::Plan,
                        crate::clock::now_ms(),
                    )
                },
                &crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .await
        })
    };
    for _ in 0..200 {
        if app
            .store
            .load_task(&id)
            .unwrap()
            .is_some_and(|t| t.question.is_some())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    app.wait_sla_tick(now_ms() + 3 * HOUR_MS).await;
    assert_eq!(
        waiter.await.unwrap().as_deref(),
        Some("keep it (reversible)")
    );
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(task.assumptions.len(), 1);
    assert_eq!(task.assumptions[0].by, "policy");
    assert!(task.decisions.iter().any(|d| d.starts_with("Policy:")));
    assert!(task
        .decisions
        .iter()
        .all(|d| !d.starts_with("Orchestrator:")));
    // A second answer, by the owner, is not a policy one.
    app.agent_answers.lock().unwrap().remove(&id);
    let mut task = task;
    record_answer_for(
        &mut task,
        None,
        &crate::engine::questions::reply::Reply {
            text: ("x").to_string(),
            default_evidence: (None).map(str::to_string),
            like: (None).map(str::to_string),
        },
        AnsweredBy::Owner,
    );
    assert!(task.assumptions.len() == 1);
}

#[tokio::test]
async fn a_default_is_refused_when_the_loop_is_not_parked_any_more() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let task = waiting_in(&repo, QuestionKind::AgentQuestion, &["A (cautious)"]);
    app.store.save_task(&task).unwrap();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    let params =
        json!({"id": task.id, "answer": "A (cautious)", "source": "policy", "evidence": "e"});
    assert!(app.handle_task_answer(params).await.is_err());
    let after = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(after.status, TaskStatus::Waiting);
}
