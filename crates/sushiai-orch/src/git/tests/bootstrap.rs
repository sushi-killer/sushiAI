use crate::git::bootstrap::bootstrap_worktree;
use crate::git::branches::unique_branch_name;
use crate::git::changes::changed_files;
use crate::git::command::run;
use crate::git::test_support::init_repo;
use crate::git::worktree::{create_worktree, worktree_path};
use std::path::PathBuf;
use std::process::Command;

/// A repo with a committed `kept.txt` and `.gitignore` ignoring the
/// given patterns, plus a worktree of it; returns (tmp, repo, worktree).
fn repo_with_worktree(ignore: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);
    std::fs::write(repo_root.join(".gitignore"), ignore).unwrap();
    std::fs::write(repo_root.join("kept.txt"), "tracked\n").unwrap();
    for args in [
        &["add", ".gitignore", "kept.txt"][..],
        &["commit", "-q", "-m", "add files"][..],
    ] {
        Command::new("git")
            .args(args)
            .current_dir(&repo_root)
            .status()
            .unwrap();
    }
    let branch = unique_branch_name(&repo_root, "Bootstrap test");
    let wt = worktree_path(&repo_root, &tmp.path().join("home"), "", &branch);
    create_worktree(&repo_root, &branch, &wt, "HEAD").unwrap();
    (tmp, repo_root, wt)
}

#[test]
fn bootstrap_without_a_worktreeinclude_copies_the_default_files_and_links_nothing() {
    let (_tmp, repo, wt) =
        repo_with_worktree(".env\n.env.local\n.claude/settings.local.json\nnode_modules/\n");
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    let defaults = [
        (".env", "SECRET=1\n"),
        (".env.local", "LOCAL=1\n"),
        (".claude/settings.local.json", "{\"local\":true}\n"),
    ];
    for (path, contents) in defaults {
        std::fs::write(repo.join(path), contents).unwrap();
    }
    let dependencies = [
        "node_modules/pkg/index.js",
        "hub/node_modules/vite/index.js",
        "hub/web/node_modules/react/index.js",
    ];
    for path in dependencies {
        let source = repo.join(path);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(source, "main checkout dependency\n").unwrap();
    }
    std::fs::create_dir_all(wt.join("hub/web")).unwrap();

    bootstrap_worktree(&repo, &wt).unwrap();

    let exclude = run(&wt, &["rev-parse", "--git-path", "info/exclude"]).unwrap();
    let exclude = std::fs::read_to_string(wt.join(exclude.trim())).unwrap();
    for (path, contents) in defaults {
        assert_eq!(std::fs::read_to_string(wt.join(path)).unwrap(), contents);
        assert!(exclude.lines().any(|line| line == format!("/{path}")));
    }
    // Nothing named node_modules is linked or created anywhere.
    let mut stack = vec![wt.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            assert_ne!(e.file_name(), "node_modules", "{:?}", e.path());
            if e.file_name() != ".git" && e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            }
        }
    }
    // Dependencies installed into the worktree stay local and ignored.
    for path in dependencies {
        let installed = wt.join(path);
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(installed, "worktree dependency\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join(path)).unwrap(),
            "main checkout dependency\n"
        );
    }
    std::fs::write(wt.join("kept.txt"), "edited\n").unwrap();
    assert_eq!(changed_files(&wt, "HEAD").unwrap(), vec!["kept.txt"]);
}

#[cfg(unix)]
#[test]
fn worktreeinclude_never_copies_through_a_symlinked_directory() {
    let (tmp, repo, wt) = repo_with_worktree("*.secret\n");
    std::fs::write(repo.join(".worktreeinclude"), "*.secret\n").unwrap();
    std::fs::create_dir_all(repo.join("conf")).unwrap();
    std::fs::write(repo.join("conf/a.secret"), "a\n").unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, wt.join("conf")).unwrap();

    bootstrap_worktree(&repo, &wt).unwrap();

    assert!(
        !outside.join("a.secret").exists(),
        "the copy left the worktree"
    );
}

#[test]
fn worktreeinclude_copies_matching_ignored_files_only() {
    let (_tmp, repo, wt) = repo_with_worktree("*.secret\n*.keep\n");
    std::fs::write(
        repo.join(".worktreeinclude"),
        "*.secret\n*.keep\nkept.txt\n",
    )
    .unwrap();
    std::fs::create_dir_all(repo.join("conf")).unwrap();
    std::fs::write(repo.join("conf/a.secret"), "a\n").unwrap();
    std::fs::write(repo.join("b.keep"), "from main\n").unwrap();
    std::fs::write(repo.join("c.txt"), "untracked, not ignored\n").unwrap();
    std::fs::write(repo.join("outside.secret"), "target\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(repo.join("outside.secret"), repo.join("link.secret")).unwrap();
    std::fs::write(wt.join("b.keep"), "already there\n").unwrap();
    std::fs::write(wt.join("kept.txt"), "edited in worktree\n").unwrap();

    bootstrap_worktree(&repo, &wt).unwrap();

    assert_eq!(
        std::fs::read_to_string(wt.join("conf/a.secret")).unwrap(),
        "a\n"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("outside.secret")).unwrap(),
        "target\n"
    );
    assert!(
        wt.join("link.secret").symlink_metadata().is_err(),
        "a symlink is skipped"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("b.keep")).unwrap(),
        "already there\n",
        "an existing worktree file is not overwritten"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("kept.txt")).unwrap(),
        "edited in worktree\n",
        "a tracked file is left alone"
    );
    assert!(!wt.join("c.txt").exists(), "not ignored, not copied");
}

#[test]
fn bootstrap_does_not_copy_an_ordinary_untracked_file() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);
    // Untracked, but NOT gitignored -- e.g. just forgotten `git add`.
    std::fs::write(repo_root.join(".env"), "SECRET=1\n").unwrap();

    let branch = unique_branch_name(&repo_root, "No bootstrap test");
    let wt_path = worktree_path(&repo_root, &tmp.path().join("home"), "", &branch);
    create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();

    bootstrap_worktree(&repo_root, &wt_path).unwrap();

    assert!(!wt_path.join(".env").exists());
}
