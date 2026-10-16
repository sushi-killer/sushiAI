use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::environment::augmented_path;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::version::codex_version;
use crate::harness::process::group_alive;
use std::time::Duration;

#[tokio::test]
async fn codex_version_timed_out_uses_the_helper_deadline_and_is_not_cached() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("codex");
    std::fs::write(&bin, "#!/bin/sh\necho $$ > \"$0.pid\"\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    // App startup normally resolves the login PATH before a task runs.
    let _ = augmented_path();
    let cancel = CancelToken::new();
    let limit = Duration::from_millis(250);
    let mut guard = Guard::helper("planner", limit);
    // Far below the fake's 30 s sleep, far above a loaded machine's
    // spawn and kill latency: it proves the helper deadline, not speed.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        codex_version(bin.to_str().unwrap(), &cancel, &mut guard),
    )
    .await
    .expect("the version probe must honor the helper deadline");
    let trip = match result {
        Err(trip) => trip,
        Ok(version) => panic!("the version probe should time out, got {version:?}"),
    };
    let mut outcome = harness::RunOutcome::default();
    trip.apply(&mut outcome);
    assert!(outcome.timed_out);
    assert_eq!(
        outcome.error,
        Some(helper_timeout_message("planner", limit))
    );
    // Under load the deadline can stop the fake before it writes its pid,
    // or between creating the pid file and writing it.
    let pid = std::fs::read_to_string(bin.with_extension("pid")).unwrap_or_default();
    if let Ok(pgid) = pid.trim().parse::<i32>() {
        assert!(!group_alive(pgid), "the version probe was not stopped");
    }

    std::fs::write(&bin, "#!/bin/sh\necho 'codex fixture'\n").unwrap();
    let mut guard = Guard::helper("planner", Duration::from_secs(10));
    let version = codex_version(bin.to_str().unwrap(), &cancel, &mut guard)
        .await
        .ok()
        .expect("the later version probe must succeed");
    assert_eq!(version.as_deref(), Some("codex fixture"));
}

#[tokio::test]
async fn cancelling_the_codex_version_probe_stops_its_process() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("codex");
    let pidfile = bin.with_extension("pid");
    std::fs::write(&bin, "#!/bin/sh\necho $$ > \"$0.pid\"\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _ = augmented_path();
    let cancel = CancelToken::new();
    let mut guard = Guard::off();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            codex_version(bin.to_str().unwrap(), &cancel, &mut guard),
            async {
                // The shell creates the file before it writes the pid.
                while std::fs::read_to_string(&pidfile).map_or(true, |p| p.trim().is_empty()) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                cancel.cancel();
            }
        )
    })
    .await
    .expect("the probe must stop when the task is cancelled");
    assert_eq!(result.ok().expect("cancellation is not a timeout"), None);
    let pgid: i32 = std::fs::read_to_string(pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!group_alive(pgid), "the cancelled probe was not stopped");
}
