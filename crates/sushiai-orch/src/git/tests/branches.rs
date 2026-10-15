use crate::delivery::git::commit::commit;
use crate::delivery::git::recovery::delete_merged_branches;
use crate::git::branches::{
    branch_of, checkout_of, default_branch, move_branch, slugify, unique_branch_name, BranchMove,
};
use crate::git::changes::{changed_files, diff_name_only, status_porcelain};
use crate::git::command::run;
use crate::git::history::{cherry_count, merge_base_of};
use crate::git::test_support::init_repo;
use crate::git::worktree::{create_worktree, worktree_path};
use std::process::Command;
use std::process::Command as StdCommand;

#[test]
fn cherry_count_ignores_patch_equivalent_commits() {
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
    let base = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "-b", "task"]);
    std::fs::write(repo.join("change.txt"), "same patch\n").unwrap();
    git(&["add", "change.txt"]);
    git(&["commit", "-q", "-m", "task change"]);
    git(&["checkout", "-q", "-b", "landed", &base]);
    std::fs::write(repo.join("change.txt"), "same patch\n").unwrap();
    git(&["add", "change.txt"]);
    git(&["commit", "-q", "-m", "landed change"]);
    let landed = git(&["rev-parse", "HEAD"]);

    assert_eq!(merge_base_of(&repo, "task", "landed").unwrap(), base);
    assert_eq!(cherry_count(&repo, &base, "task").unwrap(), 1);
    assert_eq!(cherry_count(&repo, &landed, "task").unwrap(), 0);
}

#[test]
fn move_branch_updates_an_unchecked_out_branch_and_finds_the_default() {
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
    let default = branch_of(&repo, "HEAD").unwrap();
    assert_eq!(default_branch(&repo), Some(default.clone()));
    git(&["branch", "side"]);
    git(&["checkout", "-q", "-b", "next"]);
    std::fs::write(repo.join("n.txt"), "n\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "n"]);
    let sha = git(&["rev-parse", "HEAD"]);
    assert_eq!(checkout_of(&repo, "side"), None);
    assert!(checkout_of(&repo, "next").is_some());
    assert!(matches!(
        move_branch(&repo, "side", &sha).unwrap(),
        BranchMove::Done { .. }
    ));
    assert_eq!(git(&["rev-parse", "side"]), sha);
    // Not a fast-forward any more: `side` is ahead of the default branch.
    assert!(matches!(
        move_branch(&repo, "side", &git(&["rev-parse", &default])).unwrap(),
        BranchMove::Diverged
    ));
}

// Lines an older build wrote (only the first component, e.g. `/src/` for
// `src/.wt`) are deliberately left alone: which of them the owner also
// wrote by hand cannot be told, so nothing is cleaned up.

#[test]
fn delete_merged_branches_removes_only_contained_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    let git = |args: &[&str]| {
        let status = StdCommand::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["checkout", "-q", "-b", "task/parent"]);
    git(&["branch", "task/chain-a"]);
    git(&["checkout", "-q", "-b", "task/chain-b"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "b"]);
    git(&["checkout", "-q", "task/parent"]);
    git(&["branch", "task/ahead", "task/chain-b"]);
    git(&["merge", "-q", "--no-edit", "task/chain-b"]);
    git(&["checkout", "-q", "-b", "task/unmerged"]);
    std::fs::write(repo.join("u.txt"), "u\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "u"]);
    git(&["checkout", "-q", "task/parent"]);
    git(&["branch", "other"]);
    // The checkout is on another branch, as in real use.
    git(&["checkout", "-q", "other"]);

    let pairs = |names: &[&str]| -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| (format!("id-{n}"), n.to_string()))
            .collect()
    };
    let removed = delete_merged_branches(
        &repo,
        "task/parent",
        &pairs(&[
            "task/chain-a",
            "task/chain-b",
            "task/unmerged",
            "task/parent",
        ]),
    );
    assert_eq!(removed, vec!["task/chain-a", "task/chain-b"]);
    let left = run(&repo, &["branch", "--format=%(refname:short)"]).unwrap();
    let left: Vec<&str> = left.lines().collect();
    assert!(left.contains(&"task/unmerged") && left.contains(&"task/parent"));
    assert!(left.contains(&"other") && left.contains(&"task/ahead"));
    assert!(!left.contains(&"task/chain-a") && !left.contains(&"task/chain-b"));
}

#[test]
fn slugify_lowercases_and_strips_punctuation() {
    assert_eq!(slugify("Add a Save Button!", 40), "add-a-save-button");
    assert_eq!(slugify("  weird---title__here ", 40), "weird-title-here");
    let long = "a".repeat(80);
    assert_eq!(slugify(&long, 40).len(), 40);
    assert_eq!(slugify("", 40), "task");
}

#[test]
fn create_worktree_and_status_and_commit_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);

    let branch = unique_branch_name(&repo_root, "Add a button");
    assert_eq!(branch, "task/add-a-button");
    let wt_path = worktree_path(&repo_root, &tmp.path().join("home"), "", &branch);
    let created = create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();
    assert!(created.path.exists());
    assert!(!created.base_sha.is_empty());

    // A second call with the same title must disambiguate.
    let branch2 = unique_branch_name(&repo_root, "Add a button");
    assert_eq!(branch2, "task/add-a-button-2");

    std::fs::write(wt_path.join("new_file.txt"), "content\n").unwrap();
    let changed = changed_files(&wt_path, &created.base_sha).unwrap();
    assert!(changed.contains(&"new_file.txt".to_string()));

    commit(&wt_path, "Add a button", "t1", 1).unwrap();

    // After commit, working tree is clean relative to itself but the
    // file shows up against base.
    let against_base = diff_name_only(&wt_path, &created.base_sha).unwrap();
    assert!(against_base.contains(&"new_file.txt".to_string()));
    assert!(status_porcelain(&wt_path).unwrap().is_empty());
}

#[test]
fn commit_never_runs_the_worktrees_own_hooks() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);

    let branch = unique_branch_name(&repo_root, "Hook test");
    let wt_path = worktree_path(&repo_root, &tmp.path().join("home"), "", &branch);
    let created = create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();

    // An agent-writable pre-commit hook that would prove it ran by
    // creating a marker file. `commit()` must never let it run.
    let hooks_dir = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--git-path", "hooks"])
            .current_dir(&wt_path)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let hooks_dir = wt_path.join(hooks_dir.trim());
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook_path = hooks_dir.join("pre-commit");
    std::fs::write(&hook_path, "#!/bin/sh\ntouch hook-ran.txt\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    std::fs::write(wt_path.join("new_file.txt"), "content\n").unwrap();
    commit(&wt_path, "Add a file", "t1", 1).unwrap();

    assert!(
        !wt_path.join("hook-ran.txt").exists(),
        "the worktree's pre-commit hook must not have run"
    );
    let _ = created;
}
