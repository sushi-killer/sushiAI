//! Black-box integration tests (worktree location, cleanup, restore, gc).

mod common;

use common::*;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const PASS: &str = "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";
/// Writes an uncommitted file, then hangs until the task is stopped.
const HANG: &str = "#!/bin/sh\ncat > /dev/null\necho started > AGENT_STARTED.txt\nsleep 60\n";

/// Passes while `flag` exists, hangs (like `HANG`) once it is removed.
fn switchable(flag: &Path) -> String {
    format!(
        "#!/bin/sh\ncat > /dev/null\nif [ -f {f} ]; then echo changed > CHANGED_MARKER.txt; printf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}}'; printf '%s\\n' '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"```sushi-report\\n{{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}}\\n```\"}}'; else echo started > AGENT_STARTED.txt; sleep 60; fi\n",
        f = flag.display()
    )
}

fn spawn(script_body: &str) -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", script_body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    (daemon, scripts)
}

fn create(daemon: &Daemon, repo: &Path, title: &str, start: bool) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": title,
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
            "start": start,
        }),
    )
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn canon(p: &Path) -> std::path::PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) => std::fs::canonicalize(dir).unwrap().join(name),
        _ => p.to_path_buf(),
    }
}

/// Stops a task that is hanging in its first attempt, once the agent wrote
/// its file into the worktree.
fn stop_hanging(daemon: &Daemon, id: &str, worktree: &Path) {
    wait_for("the agent started", || {
        worktree.join("AGENT_STARTED.txt").exists()
    });
    daemon.request("task.stop", serde_json::json!({"id": id}));
    poll_until(daemon, id, Duration::from_secs(30), |s| s == "stopped");
}

#[test]
fn new_worktree_lives_under_sushiai_and_done_removes_it_but_keeps_the_commit() {
    let (daemon, _scripts) = spawn(PASS);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), "Pass case", true);
    let id = task["id"].as_str().unwrap().to_string();
    let worktree = task["worktree"].as_str().unwrap().to_string();
    assert!(
        canon(Path::new(&worktree).parent().unwrap())
            == canon(&repo.path().join(".sushiai/worktrees")),
        "{worktree}"
    );
    let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
    assert!(
        exclude.lines().any(|l| l == "/.sushiai/worktrees/"),
        "{exclude}"
    );
    assert_eq!(git_out(repo.path(), &["status", "--porcelain"]), "");

    let settled = poll_task_status(&daemon, &id, Duration::from_secs(30));
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(settled["worktreeRemoved"], true, "{settled}");
    assert!(!Path::new(&worktree).exists(), "worktree dir is gone");
    let branch = settled["branch"].as_str().unwrap();
    assert_eq!(
        git_out(
            repo.path(),
            &["show", &format!("{branch}:CHANGED_MARKER.txt")]
        ),
        "changed"
    );
    let decisions = settled["decisions"].to_string();
    assert!(decisions.contains("Worktree: removed"), "{decisions}");
    daemon.shutdown_and_wait();
}

#[test]
fn archiving_a_stopped_task_saves_its_edits_and_unarchive_restores_them() {
    let (daemon, _scripts) = spawn(HANG);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), "Hang", true);
    let id = task["id"].as_str().unwrap().to_string();
    let worktree = task["worktree"].as_str().unwrap().to_string();
    let wt = Path::new(&worktree);
    stop_hanging(&daemon, &id, wt);
    std::fs::write(wt.join("README.md"), "edited\n").unwrap();
    let branch = task["branch"].as_str().unwrap().to_string();
    let head_before = git_out(repo.path(), &["rev-parse", &branch]);

    let archived = daemon.request("task.archive", serde_json::json!({"id": id}));
    assert_eq!(archived["worktreeRemoved"], true, "{archived}");
    assert!(!wt.exists());
    assert_eq!(git_out(repo.path(), &["rev-parse", &branch]), head_before);
    let wip = format!("refs/orchd/wip/{id}");
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{wip}:AGENT_STARTED.txt")]),
        "started"
    );
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{wip}:README.md")]),
        "edited"
    );

    let back = daemon.request("task.unarchive", serde_json::json!({"id": id}));
    assert!(back.get("worktreeRemoved").is_none(), "{back}");
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "edited\n"
    );
    assert!(wt.join("AGENT_STARTED.txt").exists());
    assert_eq!(git_out(wt, &["rev-parse", "HEAD"]), head_before);
    daemon.shutdown_and_wait();
}

#[test]
fn starting_a_task_whose_worktree_is_gone_recreates_it() {
    let (daemon, _scripts) = spawn(HANG);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), "Restart", true);
    let id = task["id"].as_str().unwrap().to_string();
    let worktree = task["worktree"].as_str().unwrap().to_string();
    let wt = Path::new(&worktree);
    stop_hanging(&daemon, &id, wt);
    std::fs::write(wt.join("KEEP.txt"), "mine\n").unwrap();
    // The worktree goes away behind orchd's back (a manual cleanup).
    let branch = task["branch"].as_str().unwrap();
    let saved = Command::new("git")
        .args(["worktree", "remove", "--force", &worktree])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(saved.success());
    assert!(!wt.exists());
    let _ = branch;

    daemon.request("task.start", serde_json::json!({"id": id}));
    wait_for("the worktree is back", || {
        wt.join("AGENT_STARTED.txt").exists()
    });
    assert!(wt.join(".git").exists());
    let now = daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        now["decisions"].to_string().contains("Worktree: recreated"),
        "{now}"
    );
    daemon.request("task.stop", serde_json::json!({"id": id}));
    daemon.shutdown_and_wait();
}

#[test]
fn delete_removes_the_worktree_and_the_branch() {
    let (daemon, _scripts) = spawn(HANG);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), "Doomed", true);
    let id = task["id"].as_str().unwrap().to_string();
    let wt = task["worktree"].as_str().unwrap().to_string();
    let branch = task["branch"].as_str().unwrap().to_string();
    wait_for("the agent started", || {
        Path::new(&wt).join("AGENT_STARTED.txt").exists()
    });
    daemon.request("task.delete", serde_json::json!({"id": id}));
    assert!(!Path::new(&wt).exists());
    let branches = git_out(repo.path(), &["branch", "--list", &branch]);
    assert_eq!(branches, "");
    daemon.shutdown_and_wait();
}

#[test]
fn gc_removes_done_and_archived_worktrees_but_never_running_or_unknown_ones() {
    let flag_dir = tempfile::tempdir().unwrap();
    let flag = flag_dir.path().join("pass");
    std::fs::write(&flag, "").unwrap();
    let (daemon, _scripts) = spawn(&switchable(&flag));
    let repo = init_git_repo();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    };

    // A done task whose worktree is back, as an older orchd left it.
    let done = create(&daemon, repo.path(), "Done", true);
    let done_id = done["id"].as_str().unwrap().to_string();
    let done_wt = done["worktree"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &done_id, Duration::from_secs(30));
    assert_eq!(settled["status"], "done", "{settled}");
    git(&[
        "worktree",
        "add",
        "-q",
        &done_wt,
        done["branch"].as_str().unwrap(),
    ]);
    std::fs::remove_file(&flag).unwrap();

    // A task that is still running keeps its worktree.
    let running = create(&daemon, repo.path(), "Running", true);
    let running_id = running["id"].as_str().unwrap().to_string();
    let running_wt = running["worktree"].as_str().unwrap().to_string();
    wait_for("running agent", || {
        Path::new(&running_wt).join("AGENT_STARTED.txt").exists()
    });

    // An archived stopped task with edits (its worktree is put back, as an
    // older orchd would have left it).
    let stopped = create(&daemon, repo.path(), "Stopped", true);
    let stopped_id = stopped["id"].as_str().unwrap().to_string();
    let stopped_wt = stopped["worktree"].as_str().unwrap().to_string();
    stop_hanging(&daemon, &stopped_id, Path::new(&stopped_wt));
    std::fs::write(Path::new(&stopped_wt).join("big.bin"), vec![7u8; 50_000]).unwrap();
    // Archive while the record still says stopped but the loop is gone; gc
    // is what removes it, so archive through the record instead.
    daemon.request("task.archive", serde_json::json!({"id": stopped_id}));
    assert!(!Path::new(&stopped_wt).exists());
    git(&[
        "worktree",
        "add",
        "-q",
        &stopped_wt,
        stopped["branch"].as_str().unwrap(),
    ]);
    std::fs::write(Path::new(&stopped_wt).join("big.bin"), vec![7u8; 50_000]).unwrap();

    // A worktree no task knows about.
    let unknown = repo.path().join(".sushiai/worktrees/unknown");
    git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "stray",
        unknown.to_str().unwrap(),
    ]);

    let dry = daemon.request("worktrees.gc", serde_json::json!({"dryRun": true}));
    let action = |v: &serde_json::Value, path: &str| -> String {
        v["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| canon(Path::new(e["path"].as_str().unwrap())) == canon(Path::new(path)))
            .map(|e| e["action"].as_str().unwrap().to_string())
            .unwrap_or_default()
    };
    assert_eq!(action(&dry, &stopped_wt), "would-remove", "{dry}");
    assert_eq!(action(&dry, &running_wt), "kept", "{dry}");
    assert_eq!(action(&dry, &done_wt), "would-remove", "{dry}");
    assert_eq!(action(&dry, unknown.to_str().unwrap()), "unknown", "{dry}");
    assert!(dry["freedBytes"].as_u64().unwrap() >= 50_000, "{dry}");
    assert!(Path::new(&stopped_wt).exists(), "dry run changes nothing");

    // The CLI reaches the same RPC.
    let cli = Command::new(sushiai_bin())
        .args(["orch", "gc", "--dry-run"])
        .env("SUSHIAI_HOME", daemon.home.path())
        .output()
        .unwrap();
    assert!(cli.status.success(), "{cli:?}");
    let printed: serde_json::Value =
        serde_json::from_slice(&cli.stdout).expect("gc prints its report as JSON");
    assert_eq!(printed["dryRun"], true);
    assert!(Path::new(&stopped_wt).exists());

    let real = Command::new(sushiai_bin())
        .args(["orch", "gc"])
        .env("SUSHIAI_HOME", daemon.home.path())
        .output()
        .unwrap();
    assert!(real.status.success(), "{real:?}");
    let report: serde_json::Value = serde_json::from_slice(&real.stdout).unwrap();
    assert_eq!(action(&report, &stopped_wt), "removed", "{report}");
    assert!(report["freedBytes"].as_u64().unwrap() >= 50_000, "{report}");
    assert!(!Path::new(&stopped_wt).exists());
    assert!(Path::new(&running_wt).exists(), "a running task keeps it");
    assert!(unknown.exists(), "an unknown worktree is never removed");
    assert!(!Path::new(&done_wt).exists());
    assert_eq!(action(&report, &done_wt), "removed", "{report}");
    let wip = format!("refs/orchd/wip/{stopped_id}");
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{wip}:big.bin")]).len(),
        50_000
    );
    daemon.request("task.stop", serde_json::json!({"id": running_id}));
    daemon.shutdown_and_wait();
}
