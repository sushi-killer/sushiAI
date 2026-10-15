use crate::git::changes::{
    diff_trees, diff_trees_name_only, fold_numstat, own_changes_between, status_short,
    worktree_tree,
};
use crate::git::command::run;
use crate::git::test_support::init_repo;
use std::process::Command as StdCommand;

#[test]
fn fold_numstat_counts_binary_files_as_files_without_lines() {
    let stat = fold_numstat(&[
        ("a.rs".into(), Some(3), Some(1)),
        ("logo.png".into(), None, None),
        ("b.rs".into(), Some(2), Some(0)),
    ]);
    assert_eq!(
        stat,
        crate::task::attempt::DiffStat {
            files: 3,
            added: 5,
            removed: 1
        }
    );
    assert_eq!(fold_numstat(&[]), crate::task::attempt::DiffStat::default());
}

#[test]
fn worktree_tree_covers_untracked_files_but_not_ignored_and_leaves_the_index_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    std::fs::write(repo.join(".gitignore"), "artifacts/\n").unwrap();
    let before_status = status_short(&repo).unwrap();
    let first = worktree_tree(&repo).unwrap();

    std::fs::create_dir_all(repo.join("artifacts")).unwrap();
    std::fs::write(repo.join("artifacts/a.png"), "x").unwrap();
    assert_eq!(worktree_tree(&repo).unwrap(), first, "ignored file");

    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(repo.join("tests/t.txt"), "t").unwrap();
    let second = worktree_tree(&repo).unwrap();
    assert_ne!(second, first, "untracked file");
    assert_eq!(
        diff_trees_name_only(&repo, &first, &second).unwrap(),
        vec!["tests/t.txt".to_string()]
    );

    std::fs::remove_dir_all(repo.join("tests")).unwrap();
    std::fs::remove_dir_all(repo.join("artifacts")).unwrap();
    assert_eq!(status_short(&repo).unwrap(), before_status);
}

#[test]
fn worktree_tree_rehashes_same_size_edits_with_unchanged_stat_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    init_repo(repo);
    run(repo, &["config", "core.checkStat", "minimal"]).unwrap();
    run(repo, &["config", "core.trustctime", "false"]).unwrap();
    let file = repo.join("README.md");
    let times = std::fs::FileTimes::new()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800));
    std::fs::File::open(&file)
        .unwrap()
        .set_times(times)
        .unwrap();
    run(repo, &["add", "README.md"]).unwrap();
    let index = repo.join(".git/index");
    let original_index = std::fs::read(&index).unwrap();
    let before = worktree_tree(repo).unwrap();

    std::fs::write(&file, "world\n").unwrap();
    std::fs::File::open(&file)
        .unwrap()
        .set_times(times)
        .unwrap();
    let after = worktree_tree(repo).unwrap();
    assert_ne!(after, before);
    assert_eq!(
        run(repo, &["show", &format!("{after}:README.md")]).unwrap(),
        "world\n"
    );
    assert_eq!(std::fs::read(index).unwrap(), original_index);
}

#[cfg(unix)]
#[test]
fn worktree_tree_preserves_staged_ignored_paths_with_unusual_names() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    init_repo(repo);
    std::fs::write(repo.join(".gitignore"), "ignored-*\n").unwrap();
    let name = std::ffi::OsString::from("ignored- space\nfile.txt");
    #[cfg(target_os = "linux")]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        let mut bytes = name.into_vec();
        bytes.push(0xff);
        std::ffi::OsString::from_vec(bytes)
    };
    let file = repo.join(&name);
    std::fs::write(&file, "one\n").unwrap();
    run(repo, &["add", "-f", "."]).unwrap();
    let index = repo.join(".git/index");
    let original_index = std::fs::read(&index).unwrap();
    let before = worktree_tree(repo).unwrap();

    std::fs::write(file, "two\n").unwrap();
    let after = worktree_tree(repo).unwrap();
    assert_ne!(after, before);
    let mut object = std::ffi::OsString::from(format!("{after}:"));
    object.push(name);
    let content = StdCommand::new("git")
        .arg("show")
        .arg(object)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(content.status.success(), "{content:?}");
    assert_eq!(content.stdout, b"two\n");
    assert_eq!(std::fs::read(index).unwrap(), original_index);
}

#[test]
fn own_changes_between_ignores_files_that_only_the_base_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    let git = |args: &[&str]| {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let base1 = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "-b", "task"]);
    std::fs::write(repo.join("own.tsx"), "one\n").unwrap();
    let old_tree = worktree_tree(&repo).unwrap();
    // The base moves: another task lands a UI file, the task carries it.
    git(&["checkout", "-q", "-b", "other", &base1]);
    std::fs::write(repo.join("landed.tsx"), "l\n").unwrap();
    git(&["add", "landed.tsx"]);
    git(&["commit", "-q", "-m", "landed"]);
    let base2 = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "task"]);
    std::fs::write(repo.join("landed.tsx"), "l\n").unwrap();
    let now_tree = worktree_tree(&repo).unwrap();
    assert_eq!(
        diff_trees_name_only(&repo, &old_tree, &now_tree).unwrap(),
        vec!["landed.tsx".to_string()]
    );
    assert!(
        own_changes_between(&repo, &old_tree, &base1, &now_tree, &base2)
            .unwrap()
            .is_empty()
    );
    // The task's own edit to a UI file still counts.
    std::fs::write(repo.join("own.tsx"), "two\n").unwrap();
    let edited = worktree_tree(&repo).unwrap();
    assert_eq!(
        own_changes_between(&repo, &old_tree, &base1, &edited, &base2).unwrap(),
        vec!["own.tsx".to_string()]
    );
}

#[test]
fn diff_trees_holds_only_the_given_paths_and_truncates() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    std::fs::write(repo.join("a.txt"), "a1\n").unwrap();
    std::fs::write(repo.join("b.txt"), "b1\n").unwrap();
    let old = worktree_tree(&repo).unwrap();
    std::fs::write(repo.join("a.txt"), "a2\n").unwrap();
    std::fs::write(repo.join("b.txt"), "b2\n").unwrap();
    std::fs::write(repo.join("c.txt"), "c\n").unwrap();
    let new = worktree_tree(&repo).unwrap();
    let only_a = diff_trees(&repo, &old, &new, &["a.txt".to_string()], 10_000).unwrap();
    assert!(only_a.contains("+a2"), "{only_a}");
    assert!(
        !only_a.contains("b2") && !only_a.contains("c.txt"),
        "{only_a}"
    );
    assert_eq!(diff_trees(&repo, &old, &new, &[], 10_000).unwrap(), "");
    let cut = diff_trees(&repo, &old, &new, &["a.txt".to_string()], 20).unwrap();
    assert!(cut.contains("[truncated,"), "{cut}");
}
