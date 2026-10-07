//! Black-box integration tests (run files): an implement run keeps going when
//! the daemon is killed, and the restarted daemon finishes the attempt from
//! what the run left. Fake harness scripts, temporary data dirs.

mod common;

use common::*;
use std::path::Path;
use std::time::{Duration, Instant};

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-fake"}"#;
const MESSAGE: &str = r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":100}},"parent_tool_use_id":null,"session_id":"sess-fake"}"#;

/// The first run streams a message, waits `first_wait` seconds, then reports
/// $0.30 and exits 0. A later run reports $0.20 at once.
fn script(dir: &Path, first_wait: u32) -> std::path::PathBuf {
    let marker = dir.join("first-run");
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\necho '{INIT}'\n\
         if [ ! -f {m} ]; then touch {m}\necho '{MESSAGE}'\nsleep {first_wait}\n\
         echo '{{\"type\":\"result\",\"total_cost_usd\":0.3,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\nexit 0\nfi\n\
         echo '{{\"type\":\"result\",\"total_cost_usd\":0.2,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    fake_harness_script(dir, "fake-claude.sh", &body)
}

struct Setup {
    home: std::path::PathBuf,
    first: DaemonChild,
    id: String,
    worktree: String,
    pgid: i32,
    _repo: tempfile::TempDir,
}

/// Starts a daemon, creates a task and waits until the first run has
/// streamed its message to `events.jsonl` (and its pgid is saved).
fn start(home: &Path, fake_bins: &[(&str, &str)], verify: &str) -> Setup {
    let data = home.join("orchestrator");
    let socket = socket_of(home);
    let first = spawn_daemon(home, fake_bins);
    wait_for_socket(&socket);
    let mut settings = request_on(&socket, "settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    request_on(
        &socket,
        "settings.set",
        serde_json::json!({"settings": settings}),
    );
    let repo = init_git_repo();
    let task = request_on(
        &socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Survives", "goal": "g", "criteria": [], "verify": [verify]}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let events = data.join("tasks").join(&id).join("runs/1/events.jsonl");
    let start = Instant::now();
    let pgid = loop {
        let t = request_on(&socket, "task.get", serde_json::json!({"id": id}));
        let seen = std::fs::read_to_string(&events).is_ok_and(|t| t.contains("msg_1"));
        if let (true, Some(pgid)) = (seen, t["attempts"][0]["pgid"].as_i64()) {
            break pgid as i32;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "run never streamed: {t}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    Setup {
        home: home.to_path_buf(),
        first,
        id,
        worktree: task["worktree"].as_str().unwrap().to_string(),
        pgid,
        _repo: repo,
    }
}

fn restart(s: &Setup, fake_bins: &[(&str, &str)]) -> (DaemonChild, std::path::PathBuf) {
    let socket = socket_of(&s.home);
    let second = spawn_daemon(&s.home, fake_bins);
    wait_for_socket(&socket);
    (second, socket)
}

fn wait_for(
    socket: &Path,
    id: &str,
    what: &str,
    ok: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let t = request_on(socket, "task.get", serde_json::json!({"id": id}));
        if ok(&t) {
            return t;
        }
        assert!(start.elapsed() < Duration::from_secs(25), "{what}: {t}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_run_survives_a_killed_daemon_and_ends_done_without_a_requeue() {
    let scripts = tempfile::tempdir().unwrap();
    let script = script(scripts.path(), 4);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let holder = tempfile::tempdir().unwrap();
    let mut s = start(holder.path(), &fake_bins, "true");
    s.first.kill().unwrap();
    let _ = s.first.wait();
    assert!(is_alive(s.pgid), "the run must outlive the daemon");

    let (second, socket) = restart(&s, &fake_bins);
    let t = wait_for(&socket, &s.id, "attempt did not settle", |t| {
        t["attempts"][0]["status"] == "passed"
    });
    assert_eq!(
        t["attempts"].as_array().unwrap().len(),
        1,
        "no requeue: {t}"
    );
    assert_eq!(t["status"], "done", "{t}");
    assert_eq!(t["attempts"][0]["costUsd"], 0.3, "{t}");
    assert_eq!(t["attempts"][0]["sessionId"], "sess-fake", "{t}");
    assert_eq!(t["attempts"][0]["usage"]["output"], 1, "{t}");
    assert_eq!(t["costUsd"], 0.3, "{t}");
    let log = std::fs::read_to_string(
        s.home
            .join("orchestrator/tasks")
            .join(&s.id)
            .join("runs/1/events.jsonl"),
    )
    .unwrap();
    assert!(
        log.contains("msg_1") && log.contains("total_cost_usd"),
        "{log}"
    );
    // The run is over: its hook token is gone from disk too.
    assert!(
        !token_file(&s).exists(),
        "the token file must go with the run"
    );
    stop_daemon(&socket);
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&s.worktree);
}

fn token_file(s: &Setup) -> std::path::PathBuf {
    s.home
        .join("orchestrator/tasks")
        .join(&s.id)
        .join("runs/1/events.token")
}

#[test]
fn a_readopted_run_keeps_the_token_of_its_stop_hook() {
    use std::os::unix::fs::PermissionsExt;
    let scripts = tempfile::tempdir().unwrap();
    let script = script(scripts.path(), 20);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let holder = tempfile::tempdir().unwrap();
    // Verify fails, so a live token makes the hook block the agent's stop.
    let mut s = start(holder.path(), &fake_bins, "false");
    let token = std::fs::read_to_string(token_file(&s)).expect("the run files hold the token");
    let mode = std::fs::metadata(token_file(&s))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    s.first.kill().unwrap();
    let _ = s.first.wait();
    assert!(is_alive(s.pgid), "the run must outlive the daemon");

    let (second, socket) = restart(&s, &fake_bins);
    // The loop re-adopts the run: from then on the old token is known again.
    let start_wait = Instant::now();
    let answer = loop {
        let answer = request_on(
            &socket,
            "hook.stop",
            serde_json::json!({"token": token.trim()}),
        );
        if answer["decision"] == "block" || start_wait.elapsed() > Duration::from_secs(15) {
            break answer;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        answer["decision"], "block",
        "the token was not restored: {answer}"
    );
    unsafe {
        libc::killpg(s.pgid, libc::SIGKILL);
    }
    stop_daemon(&socket);
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&s.worktree);
}

#[test]
fn a_dead_run_with_no_exit_file_is_interrupted_and_requeued() {
    let scripts = tempfile::tempdir().unwrap();
    let script = script(scripts.path(), 30);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let holder = tempfile::tempdir().unwrap();
    let mut s = start(holder.path(), &fake_bins, "true");
    s.first.kill().unwrap();
    let _ = s.first.wait();
    // The run dies with the daemon: no exit file, no live process group.
    unsafe {
        libc::killpg(s.pgid, libc::SIGKILL);
    }
    let start_wait = Instant::now();
    while is_alive(s.pgid) {
        assert!(
            start_wait.elapsed() < Duration::from_secs(5),
            "run did not die"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let (second, socket) = restart(&s, &fake_bins);
    let t = wait_for(&socket, &s.id, "task did not finish", |t| {
        t["status"] == "done"
    });
    assert_eq!(t["attempts"][0]["status"], "interrupted", "{t}");
    assert_eq!(t["attempts"].as_array().unwrap().len(), 2, "{t}");
    stop_daemon(&socket);
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&s.worktree);
}
