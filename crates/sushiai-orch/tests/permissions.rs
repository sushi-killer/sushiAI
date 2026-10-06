//! Black-box integration tests (task tools and permissions): a real daemon, a
//! fake harness that holds until released and a fake stdio MCP server. A call
//! the run may not make is held as one owner question; a refusal after the
//! run and a criterion the task has no tool for each stop before another
//! attempt.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

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
        poll_until(&self.daemon, id, Duration::from_secs(30), |s| s == status)
    }

    fn token(&self, n: usize) -> String {
        let settings: serde_json::Value =
            serde_json::from_str(&self.gate_file(&format!("settings_{n}"))).unwrap();
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        command
            .split("--token '")
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
        tool: &str,
        input: serde_json::Value,
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
    let begin = Instant::now();
    while !cond() {
        assert!(begin.elapsed() < Duration::from_secs(30), "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
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

#[test]
fn a_run_gets_the_connected_tools_the_project_servers_and_its_own_mcp() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    std::fs::write(
        env.daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("mcp.json"),
        json!({"mcpServers": {"mine": {"command": "true", "args": []}}}).to_string(),
    )
    .unwrap();
    env.start(&id);
    env.wait_run(1);

    let mcp: serde_json::Value = serde_json::from_str(&env.gate_file("mcp_1")).unwrap();
    let servers: Vec<&String> = mcp["mcpServers"].as_object().unwrap().keys().collect();
    for want in ["fake", "proj", "mine", "sushiai-messages"] {
        assert!(servers.iter().any(|s| *s == want), "{want} in {servers:?}");
    }
    // The disabled Asana tool is not started.
    assert!(!servers.iter().any(|s| *s == "asana"), "{servers:?}");

    let allow = allow_list(&env, 1);
    for read in [
        "mcp__fake__get_item",
        "mcp__fake__find_item",
        "mcp__fake__post_quiet",
    ] {
        assert!(allow.contains(&read.to_string()), "{read} in {allow:?}");
    }
    assert!(
        !allow.iter().any(|a| a.contains("send_note")),
        "a write tool is never pre-allowed: {allow:?}"
    );

    // The brief lists the tools, and writes as owner-gated.
    let brief = env.gate_file("brief_1");
    assert!(brief.contains("## Tools for this task"), "{brief}");
    assert!(brief.contains("fake: read tools"), "{brief}");
    assert!(brief.contains("send_note"), "{brief}");
    assert!(brief.contains("No tool: "), "{brief}");

    env.ungate("hold");
    env.until(&id, "done");
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_write_tool_call_is_held_for_one_owner_question_and_the_answer_goes_on_in_the_same_attempt() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    env.start(&id);
    env.wait_run(1);
    let token = env.token(1);

    // A read tool runs without asking.
    let (decision, _) = verdict(&env.hook(&token, "mcp__fake__get_item", json!({})));
    assert_eq!(decision, "allow");
    assert_eq!(env.get(&id)["status"], "running");

    // Allow once: the call is held as a question, then allowed.
    let (result, waiting) = env.held(
        &token,
        &id,
        "mcp__fake__send_note",
        json!({"text": "hello"}),
        "Allow once",
    );
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert!(
        question["text"]
            .as_str()
            .unwrap()
            .starts_with("Allow mcp__fake__send_note for this task?"),
        "{question}"
    );
    assert_eq!(
        question["options"],
        json!(["Allow once", "Always for this repo", "Deny"])
    );
    assert_eq!(verdict(&result).0, "allow");
    let after = env.get(&id);
    assert_eq!(after["status"], "running", "the same attempt goes on");
    assert!(after["question"].is_null());
    assert_eq!(after["attempts"].as_array().unwrap().len(), 1);

    // A write to an outside service asks every time unless Always.
    let (result, _) = env.held(
        &token,
        &id,
        "mcp__fake__send_note",
        json!({"text": "again"}),
        "Always for this repo",
    );
    assert_eq!(verdict(&result).0, "allow");
    let (decision, _) = verdict(&env.hook(&token, "mcp__fake__send_note", json!({})));
    assert_eq!(decision, "allow", "Always is not asked again");

    // Deny tells the agent, and the same call is not asked a second time.
    let (result, _) = env.held(
        &token,
        &id,
        "WebFetch",
        json!({"url": "https://example.invalid"}),
        "Deny",
    );
    let (decision, reason) = verdict(&result);
    assert_eq!(decision, "deny");
    assert!(reason.contains("The owner denied WebFetch"), "{reason}");
    let (decision, _) = verdict(&env.hook(&token, "WebFetch", json!({})));
    assert_eq!(decision, "deny");
    assert_eq!(env.get(&id)["status"], "running");

    env.ungate("hold");
    let done = env.until(&id, "done");
    let lines = decisions(&done);
    assert!(lines.contains("Permission: allowed once: mcp__fake__send_note"));
    assert!(lines.contains("Permission: allowed for this repo: mcp__fake__send_note"));
    assert!(lines.contains("Permission: denied: WebFetch"));

    // The rule Always stored is inherited by the next task of the repo.
    env.gate("hold", "");
    let next = env.create(&[]);
    env.start(&next);
    env.wait_run(2);
    let (decision, _) = verdict(&env.hook(&env.token(2), "mcp__fake__send_note", json!({})));
    assert_eq!(decision, "allow");
    assert!(allow_list(&env, 2).contains(&"mcp__fake__send_note".to_string()));
    // A denial is for that run: WebFetch asks again in the next task.
    let (result, _) = env.held(&env.token(2), &next, "WebFetch", json!({}), "Allow once");
    assert_eq!(verdict(&result).0, "allow");
    env.ungate("hold");
    env.until(&next, "done");
    env.daemon.shutdown_and_wait();
}

#[test]
fn an_allowed_write_under_claude_goes_through_staging_and_lands_before_verify() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    env.start(&id);
    env.wait_run(1);
    let token = env.token(1);
    let worktree = env.get(&id)["worktree"].as_str().unwrap().to_string();
    let target = format!("{worktree}/.claude/skills/x/SKILL.md");

    let (result, waiting) = env.held(
        &token,
        &id,
        "Write",
        json!({"file_path": target, "content": "hi"}),
        "Allow once",
    );
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Allow .claude/skills/x/SKILL.md for this task?"));
    let (decision, reason) = verdict(&result);
    // A hook `allow` does not pass Claude Code's protected-path check, so the
    // agent is sent to the staging dir instead.
    assert_eq!(decision, "deny");
    assert!(
        reason.contains(".orchd-staging/.claude/skills/x/SKILL.md"),
        "{reason}"
    );

    // The agent writes the staged copy; an unapproved staged file is ignored.
    let staged = Path::new(&worktree).join(".orchd-staging/.claude/skills/x/SKILL.md");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, "staged body").unwrap();
    let stray = Path::new(&worktree).join(".orchd-staging/.claude/other.md");
    std::fs::write(&stray, "never approved").unwrap();

    env.ungate("hold");
    let done = env.until(&id, "done");
    // What was committed: the staged body at the real path, nothing else.
    let branch = done["branch"].as_str().unwrap();
    let repo = env.repo.path();
    assert_eq!(
        git_out(
            repo,
            &["show", &format!("{branch}:.claude/skills/x/SKILL.md")]
        )
        .trim(),
        "staged body"
    );
    let files = git_out(repo, &["ls-tree", "-r", "--name-only", branch]);
    assert!(!files.contains(".claude/other.md"), "{files}");
    assert!(!files.contains(".orchd-staging"), "{files}");
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_refusal_after_the_run_asks_once_and_the_answer_reaches_the_next_attempt() {
    let env = spawn();
    env.gate("mode_1", "deny");
    let id = env.create(&[]);
    env.start(&id);
    let waiting = env.until(&id, "waiting");
    assert_eq!(env.runs(), 1, "no second attempt before the owner answers");
    assert_eq!(implement_attempts(&waiting), 1);
    assert_eq!(waiting["question"]["kind"], "permission");
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Allow mcp__fake__send_note for this task?"));

    env.daemon
        .request("task.answer", json!({"id": id, "answer": "Allow once"}));
    let done = env.until(&id, "done");
    assert_eq!(implement_attempts(&done), 2);
    assert!(
        allow_list(&env, 2).contains(&"mcp__fake__send_note".to_string()),
        "the attempt after the answer may use the tool"
    );
    env.daemon.shutdown_and_wait();
}

#[test]
fn the_same_refusal_again_never_starts_another_attempt() {
    let env = spawn();
    env.gate("mode", "deny");
    let id = env.create(&[]);
    env.start(&id);
    env.until(&id, "waiting");
    env.daemon
        .request("task.answer", json!({"id": id, "answer": "Deny"}));
    let failed = env.until(&id, "failed");
    assert_eq!(env.runs(), 2, "one attempt, the owner's answer, one more");
    assert_eq!(implement_attempts(&failed), 2);
    assert!(decisions(&failed).contains("refused again after the owner's answer"));
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_no_tool_report_goes_to_the_owner_instead_of_a_retry() {
    let env = spawn();
    env.gate("mode_1", "notool");
    let id = env.create(&[]);
    env.start(&id);
    let waiting = env.until(&id, "waiting");
    assert_eq!(env.runs(), 1);
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert!(question["text"]
        .as_str()
        .unwrap()
        .contains("no tool for move an Asana task"));
    // The connected Asana tool is off for the task: the owner may enable it.
    assert!(question["options"]
        .as_array()
        .unwrap()
        .contains(&json!("Enable asana for this task")));

    env.daemon.request(
        "task.answer",
        json!({"id": id, "answer": "I will do it: drop the criterion"}),
    );
    let done = env.until(&id, "done");
    assert!(decisions(&done).contains("the owner will do the part that needs move an Asana task"));
    env.daemon.shutdown_and_wait();
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

#[test]
fn the_brief_check_flags_a_criterion_the_task_has_no_tool_for_before_the_first_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let gates = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-check.sh", CHECK_SCRIPT);
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("HOME", home.path().to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["tiers"]["mechanical"] = json!("claude-sonnet");
    settings["briefCheckRoute"] = json!("claude-sonnet");
    settings["chatTools"] = json!([
        {"id": "asana", "label": "Asana", "enabled": false,
         "server": {"type": "http", "url": "https://example.invalid/mcp"}},
    ]);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Move it",
            "goal": "Ship the change",
            "criteria": ["The code compiles", "The Asana task is moved to Done"],
            "verify": ["true"],
            "land": false,
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, Duration::from_secs(20), |s| s == "waiting");

    let calls = std::fs::read_to_string(gates.path().join("calls")).unwrap();
    assert_eq!(
        calls.trim(),
        "check",
        "no attempt before the answer: {calls}"
    );
    assert!(waiting["attempts"].as_array().unwrap().is_empty());
    let prompt = std::fs::read_to_string(gates.path().join("check_prompt")).unwrap();
    assert!(prompt.contains("connectedButOffForThisTask"), "{prompt}");
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert!(question["text"]
        .as_str()
        .unwrap()
        .contains("move the Asana task"));
    assert_eq!(
        question["options"],
        json!([
            "Enable asana for this task",
            "I will do it: drop the criterion",
            "Continue anyway"
        ])
    );

    daemon.request(
        "task.answer",
        json!({"id": id, "answer": "I will do it: drop the criterion"}),
    );
    let done = poll_until(&daemon, &id, Duration::from_secs(30), |s| s == "done");
    assert_eq!(done["criteria"], json!(["The code compiles"]));
    assert!(decisions(&done).contains("it is an owner action, not this task's"));
    let calls = std::fs::read_to_string(gates.path().join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|c| *c == "check").count(), 1);
    daemon.shutdown_and_wait();
}
