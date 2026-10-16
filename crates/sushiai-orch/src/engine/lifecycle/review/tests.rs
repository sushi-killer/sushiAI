use super::rounds::ReviewCycle;
use super::verdict::Judgement;
use super::{ReviewEnd, ReviewRound, Turn};
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::final_checks::BackgroundFinal;
use crate::engine::lifecycle::report::ImplementerReport;
use crate::engine::review::types::ReviewSaw;
use crate::engine::test_support::{git_in, init_git_repo, test_app};
use crate::harness::cancel::CancelToken;
use crate::policy::settings::{SandboxMode, Settings};
use crate::task::attempt::AttemptStatus;
use crate::task::review::{ReviewResult, ReviewScope, Verdict};
use crate::task::state::{Task, TaskStatus};
use crate::task::test_support::sample_task;
use std::time::Duration;

fn round(task: &Task, settings: Settings) -> ReviewRound {
    ReviewRound::new(
        RunReceipt {
            route: settings.routes[0].clone(),
            settings,
            idx: 0,
            attempt_n: 1,
            worktree: task.worktree.clone().into(),
            base_sha: "native-start-base".into(),
            run_dir: task.worktree.clone().into(),
        },
        ImplementerReport {
            final_text: String::new(),
            report: None,
            note: String::new(),
            decisions: Vec::new(),
        },
        Vec::new(),
    )
}

fn passed() -> ReviewResult {
    ReviewResult {
        verdict: Verdict::Pass,
        findings: Vec::new(),
        repeated: Vec::new(),
        severities: Vec::new(),
        details: Vec::new(),
        criteria: Vec::new(),
    }
}

#[tokio::test]
async fn review_view_uses_the_carried_base_without_rewriting_the_native_receipt() {
    let (app, _home) = test_app();
    let mut task = sample_task(
        "review-carried-base",
        TaskStatus::Running,
        AttemptStatus::Running,
    );
    task.fixture_data().base_sha = "carried-base".into();
    let mut round = round(&task, Settings::default());
    let parent = CancelToken::new();
    let mut cycle = ReviewCycle::new(
        round.run.route.clone(),
        task.base_sha.clone(),
        parent.clone(),
        parent.child(),
    );
    let result = round
        .accept_verdict(
            &app,
            &mut task,
            &mut cycle,
            Judgement {
                result: passed(),
                recorded_as_pass: false,
                saw: ReviewSaw {
                    scope: ReviewScope::Full,
                    tree: Some("reviewed-tree".into()),
                    rulings: Vec::new(),
                },
            },
        )
        .await;
    assert!(matches!(result, Ok(Turn::Finished)));
    assert_eq!(
        task.attempts[0].review_base.as_deref(),
        Some("carried-base")
    );
    assert_eq!(
        task.attempts[0].review_tree.as_deref(),
        Some("reviewed-tree")
    );
    assert_eq!(round.run.base_sha, "native-start-base");
}

#[tokio::test]
async fn failed_concurrent_finals_discard_the_review_and_join_without_stopping_the_task() {
    let (app, _home) = test_app();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());
    let mut task = sample_task(
        "review-final-race",
        TaskStatus::Running,
        AttemptStatus::Running,
    );
    task.fixture_data().repo = repo.path().to_string_lossy().into_owned();
    task.fixture_data().worktree = task.repo.clone();
    task.fixture_data().base_sha = git_in(repo.path(), &["rev-parse", "HEAD"]);
    task.fixture_data().final_verify = vec!["echo final-failed; exit 7".into()];
    let settings = Settings {
        sandbox: SandboxMode::Host,
        ..Settings::default()
    };
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let mut round = round(&task, settings);
    round.verdict = Some(passed());
    let cancel = CancelToken::new();
    let finals = BackgroundFinal::start(&app, &task, repo.path(), &round.run.settings, &cancel);
    tokio::time::timeout(Duration::from_secs(5), finals.review_cancel.cancelled())
        .await
        .unwrap();
    round.finals = Some(finals);
    let ended = round.finish(&app, &mut task, &cancel).await;
    let ReviewEnd::Finished(Some(finished)) = ended else {
        panic!("final result was not joined")
    };
    assert_eq!(finished.results[0].code, Some(7));
    assert!(task.attempts[0].review.is_none());
    assert!(task.attempts[0].review_tree.is_none());
    assert!(task
        .decisions
        .iter()
        .any(|line| line.contains("review discarded because a final check failed")));
    assert!(!cancel.is_cancelled());
    assert_eq!(task.status, TaskStatus::Running);
}
