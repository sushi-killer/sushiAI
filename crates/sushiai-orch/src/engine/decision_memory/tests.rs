use super::*;
use crate::store::overseer;

const RETRY: &str = PRE_EXISTING_RETRY;

fn base_q() -> Question {
    let mut q = Question::new(
        "cargo test fails on base deadbee: 3 failed",
        vec![RETRY.into(), PRE_EXISTING_DROP.into(), "stop".into()],
        QuestionKind::PreexistingFailure,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    q.commands = vec!["cargo test".into()];
    q
}

fn free_q(text: &str, options: &[&str]) -> Question {
    Question::new(
        text,
        options.iter().map(|o| o.to_string()).collect(),
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    )
}

#[test]
fn the_scope_and_public_api_tags_are_hard_limits() {
    let mut q = free_q("Which way?", &["a", "b"]);
    for (risk, limit) in [
        (Risk::Scope, HardLimit::ScopeChange),
        (Risk::PublicApi, HardLimit::PublicApi),
    ] {
        q.risk = vec![risk];
        assert_eq!(hard_limit(&q, "a"), Some(limit));
    }
    let tagged: Vec<Risk> = serde_json::from_str(r#"["scope","public-api"]"#).unwrap();
    assert_eq!(tagged, vec![Risk::Scope, Risk::PublicApi]);
}

fn exhausted_q() -> Question {
    Question::new(
        "Attempts keep failing with a review failure: continue, change approach, or stop?",
        vec!["continue".into(), "stop".into()],
        QuestionKind::AttemptsFailing,
        AskedBy::Implement,
        crate::clock::now_ms(),
    )
}

/// A task of `repo` that was asked `q` and answered it.
fn answered_task(repo: &str, q: &Question, answer: &str, by: AnsweredBy) -> Task {
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().repo = repo.to_string();
    record_answer_for(
        &mut task,
        Some(q),
        &crate::engine::questions::reply::Reply {
            text: (answer).to_string(),
            default_evidence: (None).map(str::to_string),
            like: (None).map(str::to_string),
        },
        by,
    );
    task
}

fn waiting_task(repo: &str, q: Question) -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.to_string();
    task.fixture_data().question = Some(q);
    task
}

/// A git repo under `dir` and its root as orchd resolves it.
async fn repo_root(app: &App, dir: &Path) -> String {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    app.resolve_repo(repo.to_str().unwrap()).await.unwrap()
}

fn remember(app: &App, repo: &str, q: &Question, answer: &str, by: AnsweredBy) {
    app.store
        .save_task(&answered_task(repo, q, answer, by))
        .unwrap();
}

/// Parks `task` the way its loop does and returns the answer it got and its
/// final copy. A question nobody answers within a moment is cancelled.
async fn park(app: &Arc<App>, task: Task) -> (Option<String>, Task) {
    app.store.save_task(&task).unwrap();
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
    let mut parked = tokio::spawn({
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        async move {
            let mut task = task;
            let answer = QuestionSlot::new(
                &TaskSignals {
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    ..TaskSignals::new()
                },
                &mut None,
            )
            .park(&app, &mut task)
            .await;
            (answer, task)
        }
    });
    match tokio::time::timeout(Duration::from_millis(1500), &mut parked).await {
        Ok(done) => done.unwrap(),
        Err(_) => {
            cancel.cancel();
            parked.await.unwrap()
        }
    }
}

fn log_kinds(app: &App) -> Vec<String> {
    overseer::read(&app.data_dir, None, None)
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

fn overseer_app() -> (Arc<App>, tempfile::TempDir) {
    let _ = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = true;
    (app, dir)
}

#[path = "tests/answers.rs"]
mod answers;
#[path = "tests/storage.rs"]
mod storage;
