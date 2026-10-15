use crate::git::command::run;
use crate::git::history::repo_toplevel;
use crate::git::test_support::init_repo;
use crate::git::worktree::{create_worktree, exclude_worktree_root, repo_id, worktree_path};
use std::path::Path;
use std::process::Command as StdCommand;

#[test]
fn a_nested_worktree_root_is_excluded_in_full_and_leaves_its_parent_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    init_repo(&repo);

    exclude_worktree_root(&repo, &repo.join("src/.wt")).unwrap();
    let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == "/src/.wt/"), "{exclude}");
    assert!(!exclude.lines().any(|l| l == "/src/"), "{exclude}");
    // Twice adds nothing.
    exclude_worktree_root(&repo, &repo.join("src/.wt")).unwrap();
    let again = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert_eq!(again.matches("/src/.wt/").count(), 1);

    std::fs::create_dir_all(repo.join("src/.wt/task")).unwrap();
    std::fs::write(repo.join("src/.wt/task/f.txt"), "x").unwrap();
    std::fs::write(repo.join("src/new.rs"), "fn main() {}\n").unwrap();
    let status = run(&repo, &["status", "--short", "--untracked-files=all"]).unwrap();
    assert!(status.contains("src/new.rs"), "{status}");
    assert!(!status.contains(".wt"), "{status}");
}

#[test]
fn worktree_root_defaults_to_a_noindex_folder_under_the_home_outside_the_repo() {
    let repo = Path::new("/Users/me/Desktop/Test");
    let home = Path::new("/h/.sushiai");
    let id = repo_id(repo);
    assert!(
        id.starts_with("Test-") && id.len() == "Test-".len() + 8,
        "{id}"
    );
    assert_eq!(id, repo_id(repo), "the id is stable");
    assert_ne!(id, repo_id(Path::new("/Users/me/Other/Test")));
    let want = home
        .join("worktrees.noindex")
        .join(&id)
        .join("task-add-a-button");
    assert_eq!(worktree_path(repo, home, "", "task/add-a-button"), want);
    assert!(want
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .ends_with("worktrees.noindex"));
    // The value older installs stored counts as the default.
    assert_eq!(
        worktree_path(repo, home, ".sushiai/worktrees", "task/add-a-button"),
        want
    );
}

#[test]
fn a_worktree_root_override_still_resolves_against_the_repo_or_as_absolute() {
    let repo = Path::new("/Users/me/Desktop/Test");
    let home = Path::new("/h/.sushiai");
    assert_eq!(
        worktree_path(repo, home, "src/.wt", "task/x"),
        Path::new("/Users/me/Desktop/Test/src/.wt/task-x")
    );
    assert_eq!(
        worktree_path(repo, home, "/tmp/wt", "task/x"),
        Path::new("/tmp/wt/task-x")
    );
}

#[test]
fn create_worktree_starts_from_the_given_base_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    init_repo(&repo_root);
    let git = |args: &[&str]| {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(&repo_root)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo_root.join("feature.txt"), "x\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "feature"]);
    let feature_sha = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "-"]);

    let wt_path = worktree_path(&repo_root, &tmp.path().join("home"), "", "task/on-feature");
    let created = create_worktree(&repo_root, "task/on-feature", &wt_path, "feature").unwrap();
    assert_eq!(created.base_sha, feature_sha);
    assert!(wt_path.join("feature.txt").exists());
}

#[test]
fn repo_toplevel_resolves_a_subdirectory_to_the_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path().join("repo");
    std::fs::create_dir_all(repo_root.join("src/nested")).unwrap();
    init_repo(&repo_root);

    let resolved = repo_toplevel(&repo_root.join("src/nested")).unwrap();
    // macOS temp dirs are often themselves a symlink (`/tmp` ->
    // `/private/tmp`); canonicalize both sides before comparing.
    assert_eq!(
        resolved.canonicalize().unwrap(),
        repo_root.canonicalize().unwrap()
    );
}
