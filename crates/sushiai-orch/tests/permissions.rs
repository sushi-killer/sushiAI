//! Black-box integration tests (task tools and permissions): a real daemon, a
//! fake harness that holds until released and a fake stdio MCP server. A call
//! the run may not make is held as one owner question; a refusal after the
//! run and a criterion the task has no tool for each stop before another
//! attempt.

mod common;

use common::harness::fake_harness_script;
use common::polling::{poll_until, wait_until};
use common::process::Daemon;
use common::repository::{git_out, init_git_repo};

use serde_json::json;
use std::path::Path;

/// Runs are numbered by `$GATES/run_<n>`. The run holds while `$GATES/hold`
/// exists, then behaves as `$GATES/mode_<n>` (or `mode`) says: `ok` writes a
/// file and completes, `deny` ends with a refused MCP call and no change,
/// `notool` reports blocked with `No tool: ...`.
const SCRIPT: &str = r##"#!/bin/sh
input="$(cat)"
n=$(ls "$GATES"/run_* 2>/dev/null | wc -l | tr -d ' ')
n=$((n+1))
printf '%s' "$input" > "$GATES/brief_$n"
cp "$2" "$GATES/argv2_$n" 2>/dev/null
for a in "$@"; do
  [ "$prev" = "--mcp-config" ] && cp "$a" "$GATES/mcp_$n"
  [ "$prev" = "--settings" ] && cp "$a" "$GATES/settings_$n"
  prev="$a"
done
touch "$GATES/run_$n"
while [ -e "$GATES/hold" ]; do sleep 0.1; done
mode=$(cat "$GATES/mode_$n" 2>/dev/null || cat "$GATES/mode" 2>/dev/null || echo ok)
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
case "$mode" in
  ok)
    echo "$n" > "out_$n.txt"
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
  deny)
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"I could not call the tool.","permission_denials":[{"tool_name":"mcp__fake__send_note","tool_use_id":"toolu_1","tool_input":{"text":"hi"}}]}'
    ;;
  notool)
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"no way to do it\",\"decisions\":[],\"question\":\"No tool: move an Asana task\"}\n```"}'
    ;;
esac
"##;

const FAKE_MCP: &str = r##"import json, sys
TOOLS = [
    {"name": "get_item", "annotations": {"readOnlyHint": True}},
    {"name": "find_item"},
    {"name": "send_note", "annotations": {"readOnlyHint": False}},
    {"name": "post_quiet", "annotations": {"readOnlyHint": True}},
]
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg["method"]
    if method == "initialize":
        result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "fake", "version": "0"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
"##;

struct Env {
    daemon: Daemon,
    gates: tempfile::TempDir,
    repo: tempfile::TempDir,
    _scripts: tempfile::TempDir,
    _home: tempfile::TempDir,
}

fn spawn() -> Env {
    let scripts = tempfile::tempdir().unwrap();
    let gates = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(scripts.path(), "fake-claude-perm.sh", SCRIPT);
    let mcp = scripts.path().join("fake_mcp.py");
    std::fs::write(&mcp, FAKE_MCP).unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".claude.json"),
        json!({"mcpServers": {"fake": {"type": "stdio", "command": "python3", "args": [mcp]}}})
            .to_string(),
    )
    .unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("HOME", home.path().to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["chatTools"] = json!([
        {"id": "fake", "label": "Fake", "enabled": true,
         "server": {"ref": "claude-json:fake"}},
        {"id": "asana", "label": "Asana", "enabled": false,
         "server": {"type": "http", "url": "https://example.invalid/mcp"}},
    ]);
    daemon.request("settings.set", json!({"settings": settings}));
    // One look at the tools, as the Settings screen does.
    daemon.request("chat.tools", json!({"refresh": true}));
    let repo = init_git_repo();
    let repo_root = git_out(repo.path(), &["rev-parse", "--show-toplevel"]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["mcpEnable"] = json!({repo_root.trim(): ["fake"]});
    daemon.request("settings.set", json!({"settings": settings}));
    std::fs::write(
        repo.path().join(".mcp.json"),
        json!({"mcpServers": {"proj": {"command": "true", "args": []}}}).to_string(),
    )
    .unwrap();
    git_out(repo.path(), &["add", ".mcp.json"]);
    git_out(repo.path(), &["commit", "-q", "-m", "project mcp"]);
    Env {
        daemon,
        gates,
        repo,
        _scripts: scripts,
        _home: home,
    }
}

impl Env {
    fn gate(&self, name: &str, text: &str) {
        std::fs::write(self.gates.path().join(name), text).unwrap();
    }

    fn ungate(&self, name: &str) {
        let _ = std::fs::remove_file(self.gates.path().join(name));
    }

    fn gate_file(&self, name: &str) -> String {
        std::fs::read_to_string(self.gates.path().join(name)).unwrap_or_default()
    }

    fn runs(&self) -> usize {
        (1..20)
            .take_while(|n| self.gates.path().join(format!("run_{n}")).exists())
            .count()
    }

    fn create(&self, criteria: &[&str]) -> String {
        let task = self.daemon.request(
            "task.create",
            json!({
                "repo": self.repo.path().to_str().unwrap(),
                "title": "Do the thing",
                "goal": "Make a change",
                "criteria": criteria,
                "verify": ["true"],
                "land": false,
                "start": false,
            }),
        );
        task["id"].as_str().unwrap().to_string()
    }

    fn start(&self, id: &str) {
        self.daemon.request("task.start", json!({"id": id}));
    }

    fn get(&self, id: &str) -> serde_json::Value {
        self.daemon.request("task.get", json!({"id": id}))
    }

    fn wait_run(&self, n: usize) {
        wait_for(&format!("run {n} started"), || {
            self.gates.path().join(format!("run_{n}")).exists()
        });
    }

    fn until(&self, id: &str, status: &str) -> serde_json::Value {
        poll_until(&self.daemon, id, |s| s == status)
    }

    fn token(&self, n: usize) -> String {
        let settings: serde_json::Value =
            serde_json::from_str(&self.gate_file(&format!("settings_{n}"))).unwrap();
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        command
            .split("SUSHIAI_ORCH_TOKEN='")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap()
            .to_string()
    }

    fn hook(&self, token: &str, tool: &str, input: serde_json::Value) -> serde_json::Value {
        self.daemon.request(
            "hook.edit",
            json!({"token": token, "payload": {"tool_name": tool, "tool_input": input}}),
        )
    }

    /// `hook.edit` blocks while the owner is asked: the call runs on its own
    /// thread, `answer` (given the question) on this one.
    fn held(
        &self,
        token: &str,
        id: &str,
        (tool, input): (&str, serde_json::Value),
        answer: &str,
    ) -> (serde_json::Value, serde_json::Value) {
        std::thread::scope(|scope| {
            let call = scope.spawn(|| self.hook(token, tool, input));
            let waiting = self.until(id, "waiting");
            self.daemon
                .request("task.answer", json!({"id": id, "answer": answer}));
            (call.join().unwrap(), waiting)
        })
    }
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    wait_until(what, || cond().then_some(()));
}

fn verdict(result: &serde_json::Value) -> (String, String) {
    let out = &result["hookSpecificOutput"];
    (
        out["permissionDecision"].as_str().unwrap_or("").to_string(),
        out["permissionDecisionReason"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    )
}

fn decisions(task: &serde_json::Value) -> String {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn implement_attempts(task: &serde_json::Value) -> usize {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count()
}

fn allow_list(env: &Env, n: usize) -> Vec<String> {
    let settings: serde_json::Value =
        serde_json::from_str(&env.gate_file(&format!("settings_{n}"))).unwrap();
    settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// The brief check run: answers by what it is asked.
const CHECK_SCRIPT: &str = r##"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You check a coding task brief"*)
    echo check >> "$GATES/calls"
    printf '%s' "$input" > "$GATES/check_prompt"
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-check"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"{\"contradiction\": false, \"conflict\": \"\", \"missingTool\": {\"criterion\": 1, \"capability\": \"move the Asana task\", \"toolId\": \"asana\"}}"}'
    ;;
  *)
    echo implement >> "$GATES/calls"
    echo changed > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
esac
"##;

#[path = "permissions/owner.rs"]
mod owner_tests;
#[path = "permissions/preflight.rs"]
mod preflight_tests;
#[path = "permissions/tools.rs"]
mod tools_tests;
