use std::path::Path;
use std::process::Command;

/// `git init` + one commit in a fresh temp dir, for `task.create` to branch
/// a worktree from.
pub fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "orch-test@example.invalid"]);
    run(&["config", "user.name", "orch test"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}

/// Keeps a fixture cargo project's build output in its own `target/`, whatever
/// the host's `~/.cargo/config.toml` says. Only a test that runs cargo itself,
/// outside orchd, needs it: orchd's own runs get their worktree's target
/// directory from `sushiai_orch::cargo_target`.
pub fn pin_cargo_target(dir: &Path) {
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::write(
        dir.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"target\"\n",
    )
    .unwrap();
}

pub fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
