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

fn run_dir_of(s: &Setup) -> std::path::PathBuf {
    s.home.join("orchestrator/tasks").join(&s.id).join("runs/1")
}

/// Asks the daemon to stop, as an update or a quit does, and waits for it.
fn stop_gracefully(s: &mut Setup) {
    stop_daemon(&socket_of(&s.home));
    let _ = s.first.wait();
}

#[test]
fn a_graceful_stop_keeps_the_token_and_key_of_a_surviving_run() {
    let scripts = tempfile::tempdir().unwrap();
    let script = script(scripts.path(), 20);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let holder = tempfile::tempdir().unwrap();
    // Verify fails, so a live token makes the hook block the agent's stop.
    let mut s = start(holder.path(), &fake_bins, "false");
    let token = std::fs::read_to_string(token_file(&s)).expect("the run files hold the token");
    let key = run_dir_of(&s).join("key");
    std::fs::write(&key, "invented-key").unwrap();

    stop_gracefully(&mut s);

    assert!(is_alive(s.pgid), "the run must outlive the daemon");
    assert!(token_file(&s).exists(), "the token file must stay");
    assert!(key.exists(), "the key file must stay");
    let (second, socket) = restart(&s, &fake_bins);
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
    assert!(key.exists(), "the key file must outlive the restart");
    unsafe {
        libc::killpg(s.pgid, libc::SIGKILL);
    }
    stop_daemon(&socket);
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&s.worktree);
}

#[test]
fn a_graceful_stop_during_a_plan_run_ends_the_planner() {
    let scripts = tempfile::tempdir().unwrap();
    let body = "#!/bin/sh\ncat > /dev/null\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-plan\"}'\nsleep 30\n";
    let script = fake_harness_script(scripts.path(), "fake-planner.sh", body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let home = tempfile::tempdir().unwrap();
    let socket = socket_of(home.path());
    let mut first = spawn_daemon(home.path(), &fake_bins);
    wait_for_socket(&socket);
    let repo = init_git_repo();
    let task = request_on(
        &socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "request": "plan something", "start": true}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start_wait = Instant::now();
    let pgid = loop {
        let t = request_on(&socket, "task.get", serde_json::json!({"id": id}));
        if let Some(pgid) = t["attempts"][0]["pgid"].as_i64() {
            break pgid as i32;
        }
        assert!(start_wait.elapsed() < Duration::from_secs(15), "{t}");
        std::thread::sleep(Duration::from_millis(50));
    };

    stop_daemon(&socket);
    let _ = first.wait();

    // Only an implement run is re-adopted: a planner left behind would run
    // beside the one the next daemon starts.
    let gone = Instant::now();
    while is_alive(pgid) {
        if gone.elapsed() > Duration::from_secs(8) {
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
            panic!("the planner outlived the daemon");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn a_graceful_stop_during_review_resumes_into_the_review() {
    let scripts = tempfile::tempdir().unwrap();
    let started = scripts.path().join("review-started");
    // The implementer reports at once. The first review hangs; the second
    // passes. The brief of a review opens with "## Review".
    let body = format!(
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in\n\"## Review\"*)\nif [ ! -f {m} ]; then touch {m}; sleep 30; fi\n\
         printf '%s\n' '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"result\":\"```sushi-review\\n{{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}}\\n```\"}}' ;;\n\
         *) echo changed > CHANGED_MARKER.txt\nprintf '%s\n' '{INIT}'\n\
         printf '%s\n' '{{\"type\":\"result\",\"total_cost_usd\":0.02,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"```sushi-report\\n{{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}}\\n```\"}}' ;;\nesac\n",
        m = started.display()
    );
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", &body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let home = tempfile::tempdir().unwrap();
    let socket = socket_of(home.path());
    let mut first = spawn_daemon(home.path(), &fake_bins);
    wait_for_socket(&socket);
    let mut settings = request_on(&socket, "settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
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
        "title": "Review survives", "goal": "g", "criteria": [], "verify": ["true"]}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start_wait = Instant::now();
    while !started.exists() {
        assert!(
            start_wait.elapsed() < Duration::from_secs(20),
            "the review never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    stop_daemon(&socket);
    let _ = first.wait();

    let on_disk = std::fs::read_to_string(
        home.path()
            .join("orchestrator/tasks")
            .join(&id)
            .join("task.json"),
    )
    .unwrap();
    let on_disk: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
    assert_eq!(
        on_disk["attempts"][0]["status"], "running",
        "a shutdown must not end the finished attempt: {on_disk}"
    );
    let second = spawn_daemon(home.path(), &fake_bins);
    wait_for_socket(&socket);
    let t = wait_for(&socket, &id, "the task did not finish", |t| {
        t["status"] == "done"
    });
    assert_eq!(
        t["attempts"].as_array().unwrap().len(),
        1,
        "no new implement attempt: {t}"
    );
    assert_eq!(t["attempts"][0]["status"], "passed", "{t}");
    stop_daemon(&socket);
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn raw_output_files_are_private_and_deleted_once_the_attempt_is_settled() {
    use std::os::unix::fs::PermissionsExt;
    let scripts = tempfile::tempdir().unwrap();
    let script = script(scripts.path(), 3);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let holder = tempfile::tempdir().unwrap();
    let s = start(holder.path(), &fake_bins, "true");
    let dir = run_dir_of(&s);
    for name in ["events.raw", "events.stderr.log"] {
        let mode = std::fs::metadata(dir.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{name}");
    }

    let socket = socket_of(&s.home);
    wait_for(&socket, &s.id, "task did not finish", |t| {
        t["status"] == "done"
    });
    let gone = Instant::now();
    while dir.join("events.raw").exists() || dir.join("events.stderr.log").exists() {
        assert!(
            gone.elapsed() < Duration::from_secs(5),
            "unredacted output was kept"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(dir.join("events.jsonl").exists(), "the redacted log stays");
    stop_daemon(&socket);
    let _ = std::fs::remove_dir_all(&s.worktree);
}
