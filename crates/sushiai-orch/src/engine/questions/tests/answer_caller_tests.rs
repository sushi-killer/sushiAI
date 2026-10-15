use crate::api::events::Event;
use crate::engine::planning::graph::subtask_request;
use crate::engine::planning::questions::ask_plan_question;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskControl;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::harness::cancel::CancelToken;
use crate::task::AnsweredBy;
use crate::task::AskedBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::TaskStatus;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::broadcast;

async fn wait_until_parked(events: &mut broadcast::Receiver<Event>, id: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Event::Task { task } = events.recv().await.unwrap() {
                if task.id == id && task.status == TaskStatus::Waiting {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn the_parked_loop_records_each_real_caller() {
    for (source, expected) in [
        (None, AnsweredBy::Owner),
        (Some("lead"), AnsweredBy::Lead),
        (Some("mcp"), AnsweredBy::Lead),
        (Some("overseer"), AnsweredBy::Overseer),
        (Some("chat"), AnsweredBy::Orchestrator),
        (Some("policy"), AnsweredBy::Policy),
    ] {
        let (app, _dir) = test_app();
        app.settings.write().unwrap().answer_policy = false;
        let mut task = task_with_status(TaskStatus::Waiting);
        task.fixture_data().question = Some(Question::new(
            "Which way?",
            vec!["B".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
            crate::clock::now_ms(),
        ));
        let id = task.id.clone();
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
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
        let mut events = app.subscribe();
        let loop_app = app.clone();
        let waiter = tokio::spawn(async move {
            let _id = task.id.clone();
            let reply = QuestionSlot::new(
                &crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .answer(&loop_app, &mut task)
            .await;
            assert_eq!(reply.as_deref(), Ok("B"));
            task
        });
        wait_until_parked(&mut events, &id).await;
        let mut params = json!({"id": id, "answer": "B"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        if expected == AnsweredBy::Policy {
            params["evidence"] = json!("SLA elapsed");
        }
        app.handle_task_answer(params).await.unwrap();
        let task = waiter.await.unwrap();
        assert_eq!(task.question_history.len(), 1);
        assert_eq!(task.question_history[0].answered_by, expected);
        if expected == AnsweredBy::Lead {
            assert!(task.decisions.contains(&"Lead: B".to_string()));
        }
        if expected != AnsweredBy::Owner {
            assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        }
        assert!(app.agent_answers.lock().unwrap().is_empty());
        app.shutdown();
    }
}

#[tokio::test]
async fn a_parked_plan_answer_records_the_lead_and_planner_context_keeps_it() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Drafting);
    let id = task.id.clone();
    app.store.save_task(&task).unwrap();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
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
    let mut events = app.subscribe();
    let loop_app = app.clone();
    let loop_id = id.clone();
    let waiter = tokio::spawn(async move {
        ask_plan_question(
            &loop_app,
            &loop_id,
            crate::task::Question {
                risk: Vec::new(),
                default_option: None,
                ..crate::task::Question::new(
                    "Which plan?",
                    vec!["B".into()],
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
    });
    wait_until_parked(&mut events, &id).await;
    app.handle_task_answer(json!({"id": id, "answer": "B", "source": "lead"}))
        .await
        .unwrap();
    assert_eq!(waiter.await.unwrap().as_deref(), Some("B"));
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(task.question_history[0].answered_by, AnsweredBy::Lead);
    assert!(task.decisions.contains(&"Lead: B".to_string()));
    assert!(subtask_request(&task, "Build it").contains("Lead: B"));
    assert_eq!(app.take_agent_answer(&id), Some(AnsweredBy::Lead));
    app.shutdown();
}
