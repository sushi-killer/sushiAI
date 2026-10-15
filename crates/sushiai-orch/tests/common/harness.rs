use super::process::Daemon;
use std::path::{Path, PathBuf};

/// A stand-in codex that answers in prose, so a review routed to codex gives
/// no verdict, written into `dir` as `fake-codex.sh`.
pub fn fake_codex_script(dir: &Path) -> PathBuf {
    let body = "#!/bin/sh\ncat > /dev/null\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Looks fine to me.\"}}'\n";
    fake_harness_script(dir, "fake-codex.sh", body)
}

/// The claude binary of a test that did not name one: it fails at once, so the
/// test shows the mistake instead of calling a real model. Written into `dir`
/// as `unstubbed-claude.sh`.
pub fn unstubbed_claude_script(dir: &Path) -> PathBuf {
    let body = "#!/bin/sh\ncat > /dev/null\necho 'test reached the claude harness without a stub' >&2\nexit 1\n";
    fake_harness_script(dir, "unstubbed-claude.sh", body)
}

pub fn fake_harness_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// A fake Claude harness that exercises the *real* Stop hook path: it reads
/// the `--settings` file it was given, extracts the hook command the orchestrator
/// wired up, and actually runs `sushiai orch hook stop` against the live daemon
/// with a synthetic payload on stdin -- the same way the real Claude CLI
/// would, just without a real model in the loop.
pub const FAKE_CLAUDE_HOOK_SCRIPT: &str = r#"#!/usr/bin/env node
// First output before anything else runs: the stall clock must not depend on
// how long node takes to load modules on a busy machine.
console.log(JSON.stringify({ type: 'system', subtype: 'init', session_id: 'sess-fake' }));

const fs = require('fs');
const { execSync } = require('child_process');

const args = process.argv.slice(2);
const settingsPath = args[args.indexOf('--settings') + 1];
const settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
const hookCmd = settings.hooks.Stop[0].hooks[0].command;

try { fs.readFileSync(0); } catch (e) {}

fs.writeFileSync('CHANGED_MARKER.txt', 'changed\n');

const payload = JSON.stringify({
  session_id: 's',
  stop_hook_active: false,
  last_assistant_message: 'done',
});
let hookOutput = '';
try {
  hookOutput = execSync(hookCmd, { input: payload }).toString();
} catch (e) {
  hookOutput = (e.stdout || '').toString();
}

// Simulate the agent reading the hook's block reason and fixing it before
// actually finishing the session -- inside the worktree, so the fix is
// itself part of the diff the verify-result cache keys on (spec item 14):
// the hook's failing verify and the post-session gate's re-check must see
// two different diffs, not reuse a stale cached failure.
fs.writeFileSync('verified-marker.txt', 'fixed\n');

console.log(JSON.stringify({
  type: 'result',
  total_cost_usd: 0.01,
  usage: { input_tokens: 1, output_tokens: 1 },
  result: 'stop hook said: ' + hookOutput.trim(),
}));
"#;

/// A fake planner/implement harness in one script: it can tell which stage
/// it was invoked for from the brief text on its own stdin (a plan brief is
/// the only one that ever mentions "sushi-plan"), and replies with a
/// ```sushi-plan fenced draft -- with one question when `PLAN_ASK_QUESTION`
/// is set, none otherwise -- or a normal ```sushi-report implement reply.
// `printf '%s\n' "$var"` rather than `echo '...'`: this machine's `/bin/sh`
// (bash running in sh-compatibility mode) has its builtin `echo` interpret
// backslash escapes like `\n` by default, which would silently split a
// single JSON line's embedded `\n` into a real newline -- breaking the line
// framing the harness reads on stdout. `printf`'s `%s` does no such
// interpretation on its argument, only on the (separate, literal) format
// string.
pub const FAKE_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You review a coding task plan"*)
    printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    if [ -n "$PLAN_ASK_QUESTION" ]; then
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[{\"text\":\"Which default theme?\",\"options\":[\"light\",\"dark\"]}]}\n```"}'
    else
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
    fi
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

/// First attempt leaves FIRST (verify wants SECOND) and a handoff; the
/// retry creates SECOND. Every invocation's argv goes to $ARGS_LOG.
pub const FAKE_RETRY_SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$ARGS_LOG"
call=$(wc -l < "$ARGS_LOG" | tr -d ' ')
cat > "$ARGS_LOG.brief.$call"
case " $* " in
  *" --resume "*)
    if [ "$REJECT_RESUME" = 1 ]; then echo 'session not found' >&2; exit 1; fi ;;
esac
if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"tried the quick fix\",\"decisions\":[],\"question\":\"\"}\n```"}'
case " $* " in *" --resume "*) json=$(printf '%s' "$json" | sed 's/total_cost_usd":0.01/total_cost_usd":0.02/') ;; esac
printf '%s\n' "$json"
"#;

pub const FAKE_CLAUDE_PASS: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CLAUDE_ARGS\"\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

/// Points the cheap (`mechanical`) tier at a Claude route, so work routed
/// to it runs on the fake Claude harness the test set up.
pub fn cheap_route_on_claude(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["routes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "claude-cheap", "label": "Claude cheap", "harness": "claude", "model": "sonnet",
        }));
    settings["tiers"]["mechanical"] = serde_json::json!("claude-cheap");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
}

/// One fake playing every role of a task whose review always FAILs: the
/// implementer edits a file and reports, the reviewer (brief opens with
/// "## Review") fails it, and the orchestrator's accept judge (its brief
/// asks for `{"accept": true}`) records the call in $LOG_DIR/judge.called
/// and answers `{"accept": $ACCEPT_REPLY}`.
pub const FAKE_FAILING_REVIEW_SCRIPT: &str = r###"#!/bin/sh
input="$(cat)"
case "$input" in
  '## Route the review'*) exit 0 ;;
  "## Review"*)
    printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"needs work\"]}\n```"}' ;;
  *'"accept": true'*)
    touch "$LOG_DIR/judge.called"
    printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.02,\"result\":\"{\\\"accept\\\": $ACCEPT_REPLY}\"}" ;;
  *)
    echo changed > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;
