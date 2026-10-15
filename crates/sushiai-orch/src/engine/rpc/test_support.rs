use crate::engine::answer_policy::record_policy_answer;

use crate::engine::verification::base::questions::pre_existing_question;

use crate::engine::verification::base::answers::record_owner_base_check;

use crate::engine::questions::answers::record_answer;

use crate::engine::questions::reply::Reply;

use crate::engine::state::App;

#[cfg(test)]
use crate::engine::test_support::task_with_status;

#[cfg(test)]
use crate::engine::test_support::test_app;

use crate::task::AnsweredBy;

use crate::task::AskedBy;

use crate::task::Check;

use crate::task::Question;

use crate::task::QuestionKind;

use crate::task::Task;

use crate::task::TaskStatus;

use crate::task::VerifyOutcome;

use crate::task::PRE_EXISTING_RETRY;

use serde_json::json;

use tokio::sync::oneshot;

pub(in crate::engine::rpc) fn waiting(app: &App, kind: QuestionKind, options: &[&str]) -> String {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().question = Some(Question::new(
        "Allow it?",
        options.iter().map(|o| o.to_string()).collect(),
        kind,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    app.store.save_task(&task).unwrap();
    task.id.clone()
}

pub(in crate::engine::rpc) fn park_permission(app: &App, id: &str) -> oneshot::Receiver<Reply> {
    let (tx, rx) = oneshot::channel();
    app.permission_waits
        .lock()
        .unwrap()
        .insert(id.to_string(), tx);
    rx
}

pub(in crate::engine::rpc) async fn answer(
    app: &App,
    id: &str,
    text: &str,
    source: Option<&str>,
) -> Result<(), String> {
    let mut params = json!({"id": id, "answer": text});
    if let Some(s) = source {
        params["source"] = json!(s);
        if s == "policy" {
            params["evidence"] = json!("waiting SLA elapsed");
        }
    }
    app.handle_task_answer(params).await.map(|_| ())
}

/// A waiting base-check question about `command`, the task's request being `request`.
pub(in crate::engine::rpc) fn waiting_base_check(
    app: &App,
    command: &str,
    request: Option<&str>,
) -> String {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().request = request.map(str::to_string);
    task.fixture_data().question = Some(
        crate::engine::verification::base::questions::pre_existing_question(
            &[VerifyOutcome {
                command: command.to_string(),
                code: Some(1),
                tail: "boom".to_string(),
                ms: 1,
                ..Default::default()
            }],
            "0123456789",
        ),
    );
    app.store.save_task(&task).unwrap();
    task.id.clone()
}

/// A waiting `Impossible` question about criterion 0, which carries `npm test`.
pub(in crate::engine::rpc) fn waiting_impossible(app: &App) -> String {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().criteria =
        vec!["Tests pass -- check: npm test".into(), "Docs exist".into()];
    task.fixture_data().checks = vec![Check {
        criterion: 0,
        run: "npm test".into(),
        baseline: None,
    }];
    let mut question = Question::new(
        "Criterion 0 cannot be met as written: x. Evidence: y",
        vec!["drop criterion".into(), "retry".into(), "stop".into()],
        QuestionKind::Impossible,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    question.criterion = Some(0);
    task.fixture_data().question = Some(question);
    app.store.save_task(&task).unwrap();
    task.id.clone()
}

pub(in crate::engine::rpc) async fn recorded_caller(
    source: Option<&str>,
    caller: &str,
    line: &str,
) {
    let (app, _dir) = test_app();
    let id = waiting(&app, QuestionKind::AgentQuestion, &["use B"]);
    answer(&app, &id, "use B", source).await.unwrap();
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert!(
        task.decisions.iter().any(|d| d == line),
        "{:?}",
        task.decisions
    );
    let value = serde_json::to_value(&task).unwrap();
    assert_eq!(value["questionHistory"][0]["answeredBy"], caller);
    assert_eq!(value["questionHistory"][0]["answer"], "use B");
    assert_eq!(task.question_history.len(), 1);
    if caller != "owner" {
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
    }
    app.shutdown();
}

pub(in crate::engine::rpc) fn base_retry_task(commands: &[&str]) -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    let failed: Vec<VerifyOutcome> = commands
        .iter()
        .map(|command| VerifyOutcome {
            command: (*command).into(),
            code: Some(1),
            tail: "fails".into(),
            ..Default::default()
        })
        .collect();
    task.fixture_data().question = Some(pre_existing_question(&failed, &task.base_sha));
    task
}

pub(in crate::engine::rpc) fn record_base_retry(
    task: &mut Task,
    commands: &[&str],
    caller: AnsweredBy,
) {
    let mut question = task.question.clone().unwrap();
    question.commands = commands.iter().map(|c| (*c).to_string()).collect();
    if matches!(caller, AnsweredBy::Policy | AnsweredBy::Judge) {
        let by = if caller == AnsweredBy::Policy {
            "policy"
        } else {
            "judge"
        };
        record_policy_answer(task, &question, PRE_EXISTING_RETRY, by, "fails on base", 1);
    } else {
        record_answer(task, Some(&question), PRE_EXISTING_RETRY, caller);
        let sha = task.base_sha.clone();
        record_owner_base_check(task, &question.commands, &sha, PRE_EXISTING_RETRY, false);
    }
}
