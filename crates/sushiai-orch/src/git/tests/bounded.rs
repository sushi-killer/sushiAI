use crate::git::command::run_bounded;
use std::process::Command;
use std::time::Duration;

fn repo_with_hanging_fsmonitor() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let sh = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success();
        assert!(ok, "{args:?}");
    };
    sh(&["init", "-q"]);
    // The hook leaves an index.lock behind, then hangs.
    let hook = dir.path().join("hook.sh");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ntouch '{}/.git/index.lock'\nsleep 60\n",
            dir.path().display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    sh(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    dir
}

#[test]
fn a_hung_git_is_stopped_with_sigterm_and_its_stale_index_lock_removed() {
    let dir = repo_with_hanging_fsmonitor();
    let err = run_bounded(
        dir.path(),
        &["status"],
        Duration::from_millis(500),
        Duration::from_secs(3),
    )
    .unwrap_err();
    assert!(err.0.contains("timed out"), "{err}");
    assert!(!dir.path().join(".git/index.lock").exists());
}

#[test]
fn a_finished_git_returns_its_output_with_stdin_closed() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_bounded(
        dir.path(),
        &["--version"],
        Duration::from_secs(30),
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(out.starts_with("git version"), "{out}");
}

#[test]
fn a_grandchild_holding_the_output_pipe_does_not_hang_the_caller_after_git_exits() {
    let dir = tempfile::tempdir().unwrap();
    // The alias prints, then leaves a background sleep holding stdout and stderr.
    let pidfile = dir.path().join("grandchild.pid");
    let alias = format!(
        "alias.bg=!sleep 60 & echo $! > '{}'; echo hi",
        pidfile.display()
    );
    let out = run_bounded(
        dir.path(),
        &["-c", &alias, "bg"],
        Duration::from_secs(30),
        Duration::from_millis(500),
    )
    .unwrap();
    assert_eq!(out.trim(), "hi");
    // The call returned while the grandchild still held the pipes.
    let pid = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .to_string();
    let alive = Command::new("kill")
        .args(["-0", &pid])
        .status()
        .unwrap()
        .success();
    let _ = Command::new("kill").arg(&pid).status();
    assert!(alive, "run_bounded waited for the grandchild to exit");
}
