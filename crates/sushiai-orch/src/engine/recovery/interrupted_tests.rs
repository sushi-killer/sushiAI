use super::interrupted::*;
use crate::harness::run_files::RunFate;
use crate::store::Store;
use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, TaskStatus};
use std::fs;

#[test]
fn restart_recovery_marks_running_attempts_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let running_task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
    let done_task = sample_task("done", TaskStatus::Done, AttemptStatus::Passed);
    store.save_task(&running_task).unwrap();
    store.save_task(&done_task).unwrap();
    let run_dir = store.run_dir(&running_task.id, running_task.attempts[0].n);
    fs::create_dir_all(run_dir.join("b")).unwrap();
    for file in ["key", "mcp.json", "b/mcp.json"] {
        fs::write(run_dir.join(file), "synthetic secret").unwrap();
    }
    assert!(
        crate::engine::recovery::interrupted::tasks(&store, |_, _| RunFate::Resume, |_, _| {})
            .unwrap()
            .is_empty()
    );
    for file in ["key", "mcp.json", "b/mcp.json"] {
        assert!(run_dir.join(file).exists());
    }

    let recovered =
        crate::engine::recovery::interrupted::tasks(&store, |_, _| RunFate::Interrupted, |_, _| {})
            .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, "running");
    assert!(tasks(
        &store,
        |_, _| RunFate::Interrupted,
        |_, _| panic!("already settled attempt")
    )
    .unwrap()
    .is_empty());

    let reloaded = store.load_task("running").unwrap().unwrap();
    assert_eq!(reloaded.status, TaskStatus::Queued);
    assert_eq!(reloaded.attempts[0].status, AttemptStatus::Interrupted);
    for file in ["key", "mcp.json", "b/mcp.json"] {
        assert!(!run_dir.join(file).exists());
    }

    // Untouched task stays as-is.
    let untouched = store.load_task("done").unwrap().unwrap();
    assert_eq!(untouched.status, TaskStatus::Done);
}

/// A detached group like the engine's run wrapper: `$0` is `marker`.
fn spawn_group(marker: &std::path::Path) -> std::process::Child {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c").arg("sleep 30; true").arg(marker);
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().unwrap();
    crate::procs::testing::wait_for_argv(child.id() as i32, marker);
    child
}

// ci-platform: macos-only (failed on macOS while ubuntu passed, CI runs 38931522638, 38959930467; run 38931522638)
#[test]
fn recover_interrupted_kills_a_stale_process_group_if_still_alive() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
    // Proves recovery kills the run's group rather than just marking
    // the attempt interrupted and leaving an orphan running forever.
    let run_dir = store.run_dir("running", task.attempts[0].n);
    let mut child = spawn_group(&run_dir.join("events.exit"));
    task.fixture_data().attempts[0].pgid = Some(child.id() as i32);
    store.save_task(&task).unwrap();

    crate::engine::recovery::interrupted::tasks(&store, |_, _| RunFate::Interrupted, |_, _| {})
        .unwrap();

    // try_wait reaps it: a zombie's pid still answers `kill(pid, 0)`.
    crate::procs::testing::wait_until("the stale process group to be killed on recovery", || {
        child.try_wait().unwrap().is_some()
    });
}

#[test]
fn recover_interrupted_leaves_a_reused_pgid_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
    // The saved pgid belongs to a stranger's process now.
    let mut stranger = spawn_group(std::path::Path::new("/not-a-run"));
    task.fixture_data().attempts[0].pgid = Some(stranger.id() as i32);
    store.save_task(&task).unwrap();

    crate::engine::recovery::interrupted::tasks(&store, |_, _| RunFate::Interrupted, |_, _| {})
        .unwrap();

    crate::procs::testing::holds_for(
        "a process that is not the run staying alive",
        std::time::Duration::from_secs(1),
        || stranger.try_wait().unwrap().is_none(),
    );
    // The whole group: its `sleep` child would outlive the shell and the test.
    unsafe { libc::killpg(stranger.id() as i32, libc::SIGKILL) };
    let _ = stranger.wait();
}
