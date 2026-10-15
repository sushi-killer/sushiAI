use crate::engine::verification::base::answers::answerer_label;
use crate::engine::verification::base::answers::base_check_answered;

use crate::engine::graph::dependencies::DEPENDENCY_QUESTION;

use crate::engine::verification::base::questions::pre_existing_question;

use crate::harness::cancel::CancelToken;

use crate::engine::run_control::TaskControl;

#[cfg(test)]
use crate::engine::test_support::task_with_status;

#[cfg(test)]
use crate::engine::test_support::test_app;

use crate::overseer;

use crate::task::AnsweredBy;

use crate::task::AskedBy;

use crate::task::Question;

use crate::task::QuestionKind;

use crate::task::TaskStatus;

use crate::task::VerifyOutcome;

use crate::task::PRE_EXISTING_RETRY;

use serde_json::json;

use std::sync::Arc;

use std::sync::Mutex as StdMutex;

use tokio::sync::oneshot;

use crate::engine::rpc::test_support::{
    answer, base_retry_task, park_permission, record_base_retry, recorded_caller, waiting,
    waiting_base_check,
};
#[tokio::test]
async fn a_retry_that_only_asks_to_look_again_is_open_to_agents_on_a_harness_or_daily_budget_wait()
{
    let (app, _dir) = test_app();
    for (kind, options, allow) in [
        (QuestionKind::HarnessMissing, &["retry"][..], "retry"),
        (
            QuestionKind::DailyBudget,
            &["run anyway", "stop"][..],
            "run anyway",
        ),
    ] {
        let id = waiting(&app, kind, options);
        let _rx = park_permission(&app, &id);
        let err = answer(&app, &id, allow, Some("mcp")).await;
        assert_eq!(err.is_ok(), allow == "retry", "{kind:?}: {err:?}");
    }
}

#[tokio::test]
async fn an_mcp_allow_on_an_owner_only_question_is_refused_and_it_stays_pending() {
    let (app, _dir) = test_app();
    for (kind, options, allow) in [
        (
            QuestionKind::Permission,
            &["Allow once", "Always for this repo", "Deny"][..],
            "Always for this repo",
        ),
        (
            QuestionKind::ProtectedPath,
            &["approve", "reject"][..],
            "approve",
        ),
        (QuestionKind::Budget, &["raise", "stop"][..], "raise"),
        (
            QuestionKind::DailyBudget,
            &["run anyway", "stop"][..],
            "run anyway",
        ),
    ] {
        let id = waiting(&app, kind, options);
        let mut rx = park_permission(&app, &id);
        for source in ["lead", "overseer", "mcp", "chat"] {
            let err = answer(&app, &id, allow, Some(source)).await.unwrap_err();
            assert!(err.contains("owner's to allow"), "{kind:?}: {err}");
        }
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Waiting, "{kind:?}");
        assert!(task.question.is_some(), "{kind:?} is still pending");
        assert!(rx.try_recv().is_err(), "{kind:?}: nothing was delivered");
    }
}

#[tokio::test]
async fn an_mcp_deny_or_stop_is_accepted() {
    let (app, _dir) = test_app();
    let id = waiting(&app, QuestionKind::Permission, &["Allow once", "Deny"]);
    let rx = park_permission(&app, &id);
    answer(&app, &id, "Deny", Some("mcp")).await.unwrap();
    assert_eq!(rx.await.unwrap().text, "Deny");

    let id = waiting(&app, QuestionKind::ProtectedPath, &["approve", "reject"]);
    let rx = park_permission(&app, &id);
    answer(&app, &id, "reject", Some("chat")).await.unwrap();
    assert_eq!(rx.await.unwrap().text, "reject");

    let id = waiting(&app, QuestionKind::Budget, &["raise", "stop"]);
    answer(&app, &id, "stop", Some("mcp")).await.unwrap();
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Stopped);
}

#[tokio::test]
async fn only_an_overseer_answer_is_logged_cut_to_200_chars() {
    let (app, _dir) = test_app();
    // The owner's own chat turn and an MCP client are agents, but not the overseer.
    for source in ["chat", "mcp"] {
        let id = waiting(&app, QuestionKind::PlanQuestion, &[]);
        answer(&app, &id, "stop", Some(source)).await.unwrap();
    }
    assert!(overseer::read(&app.data_dir, None, None).is_empty());
    let id = waiting(&app, QuestionKind::PlanQuestion, &[]);
    let long = format!("stop {}", "x".repeat(400));
    answer(&app, &id, &long, Some("overseer")).await.unwrap();
    let log = overseer::read(&app.data_dir, None, None);
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].kind, "answered");
    assert_eq!(log[0].task_id.as_deref(), Some(id.as_str()));
    assert_eq!(log[0].text.chars().count(), 200);
    assert!(
        log[0].text.starts_with("Plan question \u{2192} stop xxx"),
        "{}",
        log[0].text
    );
    let detail = log[0].detail.as_deref().unwrap();
    assert!(detail.contains("Allow it?") && detail.ends_with(&"x".repeat(400)));

    let id = waiting(&app, QuestionKind::PlanQuestion, &[]);
    answer(&app, &id, "stop", None).await.unwrap();
    assert_eq!(overseer::read(&app.data_dir, None, None).len(), 1);
}

#[tokio::test]
async fn an_app_allow_with_no_source_is_accepted() {
    let (app, _dir) = test_app();
    let id = waiting(&app, QuestionKind::Permission, &["Allow once", "Deny"]);
    let rx = park_permission(&app, &id);
    answer(&app, &id, "Allow once", None).await.unwrap();
    assert_eq!(rx.await.unwrap().text, "Allow once");
}

#[tokio::test]
async fn a_normal_question_is_still_answerable_through_mcp() {
    let (app, _dir) = test_app();
    let id = waiting(&app, QuestionKind::AgentQuestion, &[]);
    let (tx, rx) = oneshot::channel();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(Some(tx))),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    answer(&app, &id, "use the second design", Some("mcp"))
        .await
        .unwrap();
    assert_eq!(rx.await.unwrap().text, "use the second design");
}

#[tokio::test]
async fn a_lead_answer_records_a_lead_line_and_history_without_owner_lines() {
    recorded_caller(Some("lead"), "lead", "Lead: use B").await;
}

#[tokio::test]
async fn an_undeclared_answer_stays_the_owners() {
    recorded_caller(None, "owner", "Owner: use B").await;
    recorded_caller(Some(""), "owner", "Owner: use B").await;
}

#[tokio::test]
async fn an_overseer_answer_records_the_overseer() {
    recorded_caller(
        Some("overseer"),
        "overseer",
        "Orchestrator: Allow it? -> use B (answered by the overseer)",
    )
    .await;
}

#[tokio::test]
async fn a_chat_answer_keeps_the_orchestrator_line_and_history() {
    recorded_caller(
        Some("chat"),
        "orchestrator",
        "Orchestrator: Allow it? -> use B (answered by the orchestrator chat)",
    )
    .await;
}

#[tokio::test]
async fn a_policy_answer_keeps_its_evidence_and_policy_history() {
    recorded_caller(
        Some("policy"),
        "policy",
        "Policy: Allow it? -> use B (policy: waiting SLA elapsed)",
    )
    .await;
}

#[tokio::test]
async fn a_tasks_mcp_answer_is_recorded_as_a_lead() {
    recorded_caller(Some("mcp"), "lead", "Lead: use B").await;
}

#[tokio::test]
async fn an_unknown_caller_and_a_policy_without_evidence_are_rejected() {
    let (app, _dir) = test_app();
    let id = waiting(&app, QuestionKind::AgentQuestion, &[]);
    for source in ["owner", "judge", "other", " "] {
        let err = answer(&app, &id, "use B", Some(source)).await.unwrap_err();
        assert!(err.contains("unknown answer source"), "{err}");
    }
    let err = app
        .handle_task_answer(json!({"id": id, "answer": "use B", "source": "policy"}))
        .await
        .unwrap_err();
    assert!(err.contains("needs its evidence"), "{err}");
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Waiting);
    assert!(task.question.is_some());
    assert!(task.question_history.is_empty());
    assert!(task.decisions.is_empty());
}

#[tokio::test]
async fn a_lead_stop_records_a_lead_stop_and_closes_the_task() {
    let (app, _dir) = test_app();
    let id = waiting(&app, QuestionKind::AgentQuestion, &[]);
    answer(&app, &id, " STOP ", Some("lead")).await.unwrap();
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Stopped);
    assert!(task.question.is_none());
    assert_eq!(task.decisions, ["Lead: stop (RPC)"]);
    assert_eq!(task.question_history[0].answered_by, AnsweredBy::Lead);
}

#[tokio::test]
async fn a_lead_base_check_answer_names_the_lead() {
    let (app, _dir) = test_app();
    let id = waiting_base_check(&app, "npm run ci", None);
    answer(&app, &id, PRE_EXISTING_RETRY, Some("lead"))
        .await
        .unwrap();
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert!(task.decisions.contains(&format!(
        "Lead: base check npm run ci on deadbee -> {PRE_EXISTING_RETRY}"
    )));
    assert_eq!(answerer_label(&task), Some("Lead"));
    assert!(base_check_answered(&task, "npm run ci", &task.base_sha));
    assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
    app.shutdown();
}

#[tokio::test]
async fn every_non_owner_retry_counts_on_the_same_base_but_the_owner_can_retry() {
    let (app, _dir) = test_app();
    for caller in [
        AnsweredBy::Policy,
        AnsweredBy::Judge,
        AnsweredBy::Orchestrator,
        AnsweredBy::Overseer,
        AnsweredBy::Lead,
    ] {
        for commands in [&["a.sh"][..], &["a.sh", "b.sh"][..]] {
            let mut task = base_retry_task(commands);
            record_base_retry(&mut task, &["a.sh"], caller);
            if commands.len() > 1 {
                record_base_retry(&mut task, &["b.sh"], AnsweredBy::Lead);
            }
            app.store.save_task(&task).unwrap();
            for source in ["lead", "overseer", "chat", "mcp", "policy"] {
                let err = answer(&app, &task.id, PRE_EXISTING_RETRY, Some(source))
                    .await
                    .unwrap_err();
                assert!(err.contains("the owner decides now"), "{caller:?}: {err}");
            }
            let saved = app.store.load_task(&task.id).unwrap().unwrap();
            assert_eq!(saved.status, TaskStatus::Waiting);
            assert_eq!(saved.question_history, task.question_history);
            assert!(saved.question.is_some());
            answer(&app, &task.id, PRE_EXISTING_RETRY, None)
                .await
                .unwrap();
            let saved = app.store.load_task(&task.id).unwrap().unwrap();
            assert_eq!(
                saved.question_history.last().unwrap().answered_by,
                AnsweredBy::Owner
            );
        }
    }
    app.shutdown();
}

#[tokio::test]
async fn an_uncovered_command_or_a_new_base_can_still_get_a_non_owner_retry() {
    let (app, _dir) = test_app();
    for caller in [
        AnsweredBy::Policy,
        AnsweredBy::Judge,
        AnsweredBy::Orchestrator,
        AnsweredBy::Overseer,
        AnsweredBy::Lead,
        AnsweredBy::Owner,
    ] {
        for changed_base in [false, true] {
            let mut task = base_retry_task(&["a.sh", "b.sh"]);
            record_base_retry(&mut task, &["a.sh"], caller);
            if changed_base {
                record_base_retry(&mut task, &["b.sh"], caller);
                task.fixture_data().base_sha = "1234567890".into();
                task.fixture_data().question = Some(pre_existing_question(
                    &[
                        VerifyOutcome {
                            command: "a.sh".into(),
                            code: Some(1),
                            ..Default::default()
                        },
                        VerifyOutcome {
                            command: "b.sh".into(),
                            code: Some(1),
                            ..Default::default()
                        },
                    ],
                    &task.base_sha,
                ));
            }
            app.store.save_task(&task).unwrap();
            answer(&app, &task.id, PRE_EXISTING_RETRY, Some("lead"))
                .await
                .unwrap();
            let saved = app.store.load_task(&task.id).unwrap().unwrap();
            assert_eq!(
                saved.question_history.last().unwrap().answered_by,
                AnsweredBy::Lead
            );
        }
    }
    app.shutdown();
}

#[tokio::test]
async fn a_lead_dependency_answer_and_child_decision_name_the_lead() {
    let (app, _dir) = test_app();
    let mut parent = task_with_status(TaskStatus::Waiting);
    let mut child = task_with_status(TaskStatus::Failed);
    child.fixture_data().parent = Some(parent.id.clone());
    parent.fixture_data().question = Some(Question::new(
        format!("{DEPENDENCY_QUESTION}: child failed"),
        vec!["drop the dependency".into()],
        QuestionKind::DependencyEnded,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    app.store.save_task(&child).unwrap();
    app.store.save_task(&parent).unwrap();
    answer(&app, &parent.id, "drop the dependency", Some("lead"))
        .await
        .unwrap();
    let parent = app.store.load_task(&parent.id).unwrap().unwrap();
    let child = app.store.load_task(&child.id).unwrap().unwrap();
    assert_eq!(parent.question_history[0].answered_by, AnsweredBy::Lead);
    assert!(parent
        .decisions
        .iter()
        .any(|d| d == "Lead: drop the dependency"));
    assert!(parent.decisions.iter().all(|d| !d.starts_with("Owner:")));
    assert!(child
        .decisions
        .iter()
        .any(|d| d.starts_with("Lead: dropped from")));
    assert!(child.parent.is_none());
}
