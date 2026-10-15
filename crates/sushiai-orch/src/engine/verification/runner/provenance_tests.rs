use super::{run_one_verify_command, run_verify_commands, CheckRunner};
use crate::engine::test_support::{git_in, init_git_repo, test_app};
use crate::harness::cancel::CancelToken;

#[tokio::test]
async fn a_reused_pass_keeps_its_original_checkout_commit_and_checked_tree() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let original = git_in(&repo, &["rev-parse", "HEAD"]);
    let counter = dir.path().join("runs");
    let command = format!("echo checked >> '{}'", counter.display());
    let cancel = CancelToken::new();
    let runner = CheckRunner::new(&app, &repo, None, "base", &cancel);
    let first = run_one_verify_command(runner, &command).await;
    assert_eq!(first.code, Some(0));
    assert_eq!(first.commit.as_deref(), Some(original.as_str()));
    assert!(first.tree.is_some());
    git_in(&repo, &["commit", "--allow-empty", "-qm", "same tree"]);
    let latest = git_in(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(latest, original);
    let cached = run_one_verify_command(runner, &command).await;
    assert_eq!(cached.commit, first.commit);
    assert_eq!(cached.tree, first.tree);
    assert_eq!(cached.ms, 0);
    assert!(cached
        .decisions
        .iter()
        .any(|line| line.contains("not run again")));
    assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 1);
    let failed = run_one_verify_command(runner, "false").await;
    assert_eq!(failed.code, Some(1));
    assert_eq!(failed.commit.as_deref(), Some(latest.as_str()));
    assert_eq!(failed.tree, first.tree);
}

#[tokio::test]
async fn consecutive_commands_record_the_tree_each_actually_checks() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let original = crate::verify::passes::check_tree(&repo).await.unwrap();
    let cancel = CancelToken::new();
    let outcomes = run_verify_commands(
        CheckRunner::new(&app, &repo, None, "base", &cancel),
        &["echo changed > README.md".into(), "true".into()],
    )
    .await;
    let changed = crate::verify::passes::check_tree(&repo).await.unwrap();
    assert_ne!(original, changed);
    assert_eq!(outcomes[0].tree.as_deref(), Some(original.as_str()));
    assert_eq!(outcomes[1].tree.as_deref(), Some(changed.as_str()));
}
