use crate::engine::permissions::answers::already_answered;
use crate::engine::permissions::answers::record_permission_answer;
use crate::engine::permissions::hook::ask;
use crate::engine::run_control::HookContext;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::policy::permissions::Choice;
use crate::policy::permissions::Subject;
use crate::task::AnsweredBy;
use crate::task::TaskStatus;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

#[test]
fn an_answered_subject_stays_answered_after_the_line_is_reworded() {
    let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
    let (a, b) = (
        Subject::Path("docs/a.md".into()),
        Subject::Tool("mcp__x".into()),
    );
    assert!(!already_answered(&task, std::slice::from_ref(&a)));
    record_permission_answer(&mut task, &a, &Choice::Deny(None));
    task.fixture_data().decisions = vec!["reworded".into()];
    assert!(already_answered(&task, std::slice::from_ref(&a)));
    assert!(!already_answered(&task, &[a.clone(), b.clone()]));
    record_permission_answer(&mut task, &b, &Choice::Once);
    assert!(already_answered(&task, &[a, b]));
}

#[tokio::test]
async fn a_held_permission_refusal_keeps_the_caller_and_policy_evidence() {
    for (source, caller) in [
        (None, AnsweredBy::Owner),
        (Some("lead"), AnsweredBy::Lead),
        (Some("overseer"), AnsweredBy::Overseer),
        (Some("chat"), AnsweredBy::Orchestrator),
        (Some("policy"), AnsweredBy::Policy),
    ] {
        let (app, _dir) = test_app();
        let task = task_with_status(TaskStatus::Running);
        app.store.save_task(&task).unwrap();
        let ctx = HookContext::new(&task, 1, Path::new(&task.worktree), &task.base_sha);
        let loop_app = app.clone();
        let mut events = app.subscribe();
        let waiter = tokio::spawn(async move {
            ask(
                &loop_app,
                &ctx,
                &Subject::Tool("mcp__example__write".into()),
                "",
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        let mut params = json!({"id": task.id, "answer": "Deny"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        if caller == AnsweredBy::Policy {
            params["evidence"] = json!("waited");
        }
        app.handle_task_answer(params).await.unwrap();
        assert_eq!(waiter.await.unwrap().as_deref(), Some("Deny"));
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert!(task.question.is_none());
        assert_eq!(task.question_history[0].answered_by, caller);
        if caller == AnsweredBy::Policy {
            assert_eq!(task.assumptions[0].evidence, "waited");
            assert!(task.decisions[0].starts_with("Policy:"));
        }
        assert!(app.agent_answers.lock().unwrap().is_empty());
        app.shutdown();
    }
}
