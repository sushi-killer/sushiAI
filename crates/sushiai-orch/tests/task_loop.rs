//! Black-box integration tests (task loop): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::polling::deadline;
use common::process::Daemon;

use common::attempts::run_file;
use common::harness::{
    fake_harness_script, FAKE_CLAUDE_HOOK_SCRIPT, FAKE_CLAUDE_PASS, FAKE_RETRY_SCRIPT,
};
use common::native_sandbox::fit_sandbox;
use common::polling::{poll_task_status, poll_until, remove_dir_retrying, wait_until};
use common::process::{is_alive, socket_of, spawn_daemon, wait_for_exit};
use common::protocol::{request_on, stop_daemon, wait_for_socket};
use common::repository::init_git_repo;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Like [`FAKE_PLANNER_SCRIPT`] but the draft's own `verify` always fails,
/// for exercising the implement loop's own attempt budget on top of a
/// drafted task (spec review item P1-3: the plan attempt must never count
/// against `maxAttempts`).
const FAILING_VERIFY_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"false\"],\"questions\":[]}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

/// Waits until the daemon has saved the run's pgid. The harness script runs
/// (and touches its marker) a moment before the daemon records the group, and a
/// restart can only follow a run whose group it knows: killing the daemon in
/// that gap legitimately interrupts and requeues the attempt.
fn wait_for_saved_pgid(socket: &Path, id: &str) {
    wait_until("the run's pgid to be saved", || {
        let t = request_on(socket, "task.get", serde_json::json!({"id": id}));
        t["attempts"][0]["pgid"].is_i64().then_some(())
    });
}

/// Starts a task whose first harness run hangs, then ends the daemon with
/// `end` (the `shutdown` RPC or `task.stop` followed by `shutdown`), restarts
/// it and returns the task as the second daemon sees it once it settled or
/// after a short quiet period.
fn hang_shutdown_and_restart(stop_first: bool) -> serde_json::Value {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}; sleep 4; fi\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();

    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["planReview"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
    );
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Survives a shutdown", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    wait_until("the harness to start", || marker.exists().then_some(()));
    wait_for_saved_pgid(&socket1, &id);
    if stop_first {
        request_on(&socket1, "task.stop", serde_json::json!({"id": id}));
        wait_until("the task to stop", || {
            let t = request_on(&socket1, "task.get", serde_json::json!({"id": id}));
            (t["status"] == "stopped").then_some(())
        });
    }
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket2);
    let call = |m: &str, p: serde_json::Value| request_on(&socket2, m, p);
    let settled = if stop_first {
        // Absence check: this window gives a wrongly resumed run time to finish.
        let start = Instant::now();
        loop {
            let t = call("task.get", serde_json::json!({"id": id}));
            if t["status"] == "done" || start.elapsed() > Duration::from_secs(3) {
                break t;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    } else {
        wait_until("the task to resume and finish after the restart", || {
            let t = call("task.get", serde_json::json!({"id": id}));
            (t["status"] == "done").then_some(t)
        })
    };
    stop_daemon(&socket2);
    let _ = wait_for_exit(second);
    remove_dir_retrying(task["worktree"].as_str().unwrap());
    settled
}

/// Emits the same `Bash` call three times, a different one on the second
/// run so the two failures do not share a signature, then hangs.
const FAKE_LOOPING_CLAUDE: &str = r#"#!/bin/sh
cat > /dev/null
if [ -f RAN ]; then word=beta; else word=alpha; fi
touch RAN
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-loop"}'
for i in 1 2 3; do
  printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"id\":\"m$i\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t$i\",\"name\":\"Bash\",\"input\":{\"command\":\"echo $word\"}}]}}"
done
exec sleep 60
"#;

#[path = "task_loop/execution.rs"]
mod execution_tests;
#[path = "task_loop/recovery.rs"]
mod recovery_tests;
#[path = "task_loop/resume.rs"]
mod resume_tests;
#[path = "task_loop/safety.rs"]
mod safety_tests;
