use super::{parent, protection, LoopProgress};
use crate::engine::decision::failure;
use crate::engine::lifecycle::admission::{graph, Admission};
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::routing::Tier;
use crate::task::{FailureKind, Task, TaskStatus};
use std::sync::Arc;

fn parent_task() -> (Arc<App>, Task, tempfile::TempDir) {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut parent = task_with_status(TaskStatus::Queued);
    parent.fixture_data().repo = repo.to_string_lossy().into_owned();
    parent.fixture_data().worktree = parent.repo.clone();
    parent.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    parent.fixture_data().tier = Tier::Hard;
    parent.schedule_pr_check_repair(
        "cause".into(),
        failure(FailureKind::Verify, "named check output".into()),
    );
    let mut child = task_with_status(TaskStatus::Done);
    child.fixture_data().repo = parent.repo.clone();
    child.fixture_data().parent = Some(parent.id.clone());
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();
    (app, parent, dir)
}

#[tokio::test]
async fn a_parent_without_an_implement_attempt_gets_its_designated_own_repair_step() {
    let (app, mut task, _dir) = parent_task();
    assert!(task.attempts.is_empty());
    let signals = TaskSignals::new();
    assert!(matches!(
        graph(&app, &mut task, &signals, None).await,
        Admission::Ready { .. }
    ));
    let mut progress = LoopProgress::new(1, true);
    let mut permit = None;
    assert!(
        parent::admit(&app, &mut task, &signals, &mut progress, &mut permit)
            .await
            .is_ok()
    );
    assert_eq!(
        progress.budget, 1,
        "the repair does not raise the general attempt budget"
    );
    assert!(task.attempts.is_empty());
    assert!(task.pr_check_repair_due());
}

#[tokio::test]
async fn the_first_parent_repair_keeps_its_existing_tier_without_a_planner_reset() {
    let (app, mut task, _dir) = parent_task();
    let signals = TaskSignals::new();
    let mut progress = LoopProgress::new(1, true);
    assert!(
        protection::prepare(&app, &mut task, &signals, &mut progress, &mut None)
            .await
            .is_ok()
    );
    assert_eq!(task.tier, Tier::Hard);
    assert_eq!(progress.budget, 1);
}
