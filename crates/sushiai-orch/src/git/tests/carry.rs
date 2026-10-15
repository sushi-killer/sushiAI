use crate::delivery::git::carry::{carry_onto_moved_base, Rebase};
use crate::git::branches::branch_of;
use crate::git::test_support::init_repo;
use crate::git::worktree::{create_worktree, worktree_path};
use std::path::Path;
use std::process::Command as StdCommand;

#[test]
fn carry_onto_moved_base_moves_clean_work_and_marks_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);
    let git = |dir: &Path, args: &[&str]| {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let base = branch_of(&repo_root, "HEAD").unwrap();
    let wt = worktree_path(&repo_root, &tmp.path().join("home"), "", "task/carry");
    let created = create_worktree(&repo_root, "task/carry", &wt, "HEAD").unwrap();

    // The agent edits README and adds a file; meanwhile the base gains
    // an unrelated file.
    std::fs::write(wt.join("README.md"), "hello from the task\n").unwrap();
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();
    std::fs::write(repo_root.join("other.txt"), "base\n").unwrap();
    git(&repo_root, &["add", "."]);
    git(&repo_root, &["commit", "-q", "-m", "base moves"]);
    let moved = git(&repo_root, &["rev-parse", "HEAD"]);

    let r = carry_onto_moved_base(&wt, &base, &created.base_sha, "t1").unwrap();
    assert_eq!(
        r,
        Rebase::Moved {
            new_sha: moved.clone(),
            rewritten: false
        }
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "hello from the task\n"
    );
    assert!(wt.join("new.txt").exists() && wt.join("other.txt").exists());
    assert!(
        !git(&wt, &["status", "--porcelain"]).is_empty(),
        "work stays uncommitted"
    );
    assert_eq!(
        carry_onto_moved_base(&wt, &base, &moved, "t1").unwrap(),
        Rebase::Unchanged
    );

    // Now the base edits the same line of README.
    std::fs::write(repo_root.join("README.md"), "hello from the base\n").unwrap();
    git(&repo_root, &["commit", "-qam", "base edits README"]);
    match carry_onto_moved_base(&wt, &base, &moved, "t1").unwrap() {
        Rebase::Conflicts { files, .. } => assert_eq!(files, vec!["README.md"]),
        other => panic!("expected conflicts, got {other:?}"),
    }
    assert!(std::fs::read_to_string(wt.join("README.md"))
        .unwrap()
        .contains("<<<<<<<"));
    assert!(!git(&wt, &["rev-parse", "refs/orchd/wip/t1"]).is_empty());
}

#[test]
fn carry_onto_moved_base_skips_what_would_lose_work() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);
    let git = |dir: &Path, args: &[&str]| {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let base = branch_of(&repo_root, "HEAD").unwrap();
    let first = git(&repo_root, &["rev-parse", "HEAD"]);
    let wt = worktree_path(&repo_root, &tmp.path().join("home"), "", "task/skip");
    let created = create_worktree(&repo_root, "task/skip", &wt, "HEAD").unwrap();
    std::fs::write(repo_root.join("b.txt"), "b\n").unwrap();
    git(&repo_root, &["add", "."]);
    git(&repo_root, &["commit", "-q", "-m", "base moves"]);

    // An ignored local file the new base would start tracking.
    std::fs::write(wt.join(".gitignore"), "secret.env\n").unwrap();
    std::fs::write(wt.join("secret.env"), "mine\n").unwrap();
    std::fs::write(repo_root.join("secret.env"), "template\n").unwrap();
    git(&repo_root, &["add", "secret.env"]);
    git(&repo_root, &["commit", "-q", "-m", "track secret.env"]);
    let r = carry_onto_moved_base(&wt, &base, &created.base_sha, "t2").unwrap();
    assert!(
        matches!(r, Rebase::Skipped { ref reason } if reason.contains("secret.env")),
        "{r:?}"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("secret.env")).unwrap(),
        "mine\n"
    );
    std::fs::remove_file(wt.join("secret.env")).unwrap();
    std::fs::remove_file(wt.join(".gitignore")).unwrap();

    // Commits of the agent's own on the task branch.
    std::fs::write(wt.join("a.txt"), "a\n").unwrap();
    git(&wt, &["add", "."]);
    git(
        &wt,
        &[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "agent",
        ],
    );
    let r = carry_onto_moved_base(&wt, &base, &created.base_sha, "t2").unwrap();
    assert!(matches!(r, Rebase::Skipped { .. }), "{r:?}");
    assert!(wt.join("a.txt").exists());

    // The same branch after the new base was merged into it: the base
    // moves to that head and the branch is left as it is.
    let base_head = git(&repo_root, &["rev-parse", "HEAD"]);
    git(
        &wt,
        &[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "merge",
            "-q",
            "--no-edit",
            &base_head,
        ],
    );
    let merged = git(&wt, &["rev-parse", "HEAD"]);
    let r = carry_onto_moved_base(&wt, &base, &created.base_sha, "t2").unwrap();
    assert!(
        matches!(r, Rebase::Moved { ref new_sha, rewritten: false } if *new_sha == base_head),
        "{r:?}"
    );
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), merged);

    // A base rewritten so the task's base is no longer in its history:
    // the task's own edits are carried onto the amended head.
    git(&wt, &["reset", "-q", "--hard", &created.base_sha]);
    git(&repo_root, &["reset", "-q", "--hard", &first]);
    std::fs::write(repo_root.join("amended.txt"), "amended\n").unwrap();
    git(&repo_root, &["add", "."]);
    git(
        &repo_root,
        &["commit", "-q", "--amend", "-m", "rewritten root"],
    );
    let amended = git(&repo_root, &["rev-parse", "HEAD"]);
    std::fs::write(wt.join("mine.txt"), "mine\n").unwrap();
    std::fs::write(wt.join("README.md"), "task edit\n").unwrap();
    let r = carry_onto_moved_base(&wt, &base, &created.base_sha, "t2").unwrap();
    assert_eq!(
        r,
        Rebase::Moved {
            new_sha: amended.clone(),
            rewritten: true
        }
    );
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), amended);
    assert!(wt.join("amended.txt").exists() && wt.join("mine.txt").exists());
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "task edit\n"
    );
    assert!(!git(&wt, &["status", "--porcelain"]).is_empty());

    // The amended tip edits the line the task edits: conflicts.
    git(&wt, &["reset", "-q", "--hard", &created.base_sha]);
    std::fs::write(wt.join("README.md"), "task edit\n").unwrap();
    std::fs::write(repo_root.join("README.md"), "base edit\n").unwrap();
    git(&repo_root, &["add", "."]);
    git(
        &repo_root,
        &["commit", "-q", "--amend", "-m", "rewritten again"],
    );
    let r = carry_onto_moved_base(&wt, &base, &created.base_sha, "t2").unwrap();
    assert!(
        matches!(&r, Rebase::Conflicts { files, rewritten: true, .. } if files == &vec!["README.md".to_string()]),
        "{r:?}"
    );
    assert!(std::fs::read_to_string(wt.join("README.md"))
        .unwrap()
        .contains("<<<<<<<"));
}
