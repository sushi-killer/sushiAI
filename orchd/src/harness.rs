//! Argv construction for `claude`/`codex`, and parsing of their streamed
//! JSON events. Argv building is pure (`Vec<String>` in, testable without a
//! process); stream parsing folds one line at a time into a
//! [`RunOutcome`] as the engine reads the child's stdout.

use crate::model::{price_for, Harness, Price, SandboxMode};
use std::collections::BTreeMap;
use std::path::Path;

pub struct RunRequest<'a> {
    pub harness: Harness,
    pub worktree: &'a Path,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// Resume the given session id instead of starting fresh.
    pub resume: Option<&'a str>,
    /// Read-only review session instead of an implement session.
    pub review: bool,
    /// Claude only: paths to the run's `mcp.json` / `settings.json`.
    pub mcp_config: Option<&'a Path>,
    pub settings_path: Option<&'a Path>,
    /// Codex only: whether the sandbox has network access
    /// (`sandbox_workspace_write.network_access`).
    pub network_allowed: bool,
    /// Codex only: an MCP server (name, `{command, args, env?}`) passed as
    /// `-c mcp_servers.*` flags; Claude reads its servers from `mcp_config`.
    pub codex_mcp: Option<(&'a str, &'a serde_json::Value)>,
    /// Codex only: images attached to the prompt (`--image`); Claude opens
    /// image files itself with its Read tool.
    pub images: &'a [std::path::PathBuf],
    /// Claude only: load the checkout's own `.claude/settings{,.local}.json`
    /// (`--setting-sources project,local`). Off for a run in a checkout
    /// orchd does not own (an audited repo): those settings carry hooks,
    /// shell commands that run outside the sandbox.
    pub repo_settings: bool,
}

/// The tools an orchd agent never needs: it implements one bounded task and
/// does not delegate. `Task` and `Agent` are the subagent tool's two names.
pub const DELEGATION_TOOLS: &str = "Task,Agent,Workflow,SendMessage,ListAgents";

/// `-c mcp_servers.<name>.{command,args,env.*}` for one MCP server. JSON
/// strings and string arrays are valid TOML values as they are.
pub fn codex_mcp_flags(name: &str, server: &serde_json::Value) -> Vec<String> {
    let mut flags = vec![
        "-c".to_string(),
        format!("mcp_servers.{name}.command={}", server["command"]),
        "-c".to_string(),
        format!("mcp_servers.{name}.args={}", server["args"]),
    ];
    if let Some(env) = server.get("env").and_then(|e| e.as_object()) {
        for (key, value) in env {
            flags.push("-c".to_string());
            flags.push(format!("mcp_servers.{name}.env.{key}={value}"));
        }
    }
    flags
}

/// `claude -p --output-format stream-json --verbose --setting-sources
/// project,local --disable-slash-commands --strict-mcp-config --mcp-config
/// <mcp.json> --settings <settings.json> --permission-mode acceptEdits
/// --disallowedTools <DELEGATION_TOOLS> --permission-prompts none [--model]
/// [--effort] [--resume <id>]`; review swaps `--permission-mode acceptEdits`
/// (and the delegation-tools trim, which review never gets) for
/// `--tools Read,Grep,Glob --permission-mode plan`.
pub fn claude_argv(req: &RunRequest) -> Vec<String> {
    let mut argv = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--setting-sources".to_string(),
        if req.repo_settings {
            "project,local"
        } else {
            ""
        }
        .to_string(),
        "--disable-slash-commands".to_string(),
        "--strict-mcp-config".to_string(),
    ];
    if let Some(mcp) = req.mcp_config {
        argv.push("--mcp-config".to_string());
        argv.push(mcp.to_string_lossy().to_string());
    }
    if let Some(settings) = req.settings_path {
        argv.push("--settings".to_string());
        argv.push(settings.to_string_lossy().to_string());
    }
    if req.review {
        argv.push("--tools".to_string());
        argv.push("Read,Grep,Glob".to_string());
        argv.push("--permission-mode".to_string());
        argv.push("plan".to_string());
    } else {
        argv.push("--permission-mode".to_string());
        argv.push("acceptEdits".to_string());
        // One comma-joined value: the flag is variadic.
        argv.push("--disallowedTools".to_string());
        argv.push(DELEGATION_TOOLS.to_string());
    }
    argv.push("--permission-prompts".to_string());
    argv.push("none".to_string());
    if let Some(model) = req.model {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    if let Some(effort) = req.effort {
        argv.push("--effort".to_string());
        argv.push(effort.to_string());
    }
    if !req.review {
        if let Some(session) = req.resume {
            argv.push("--resume".to_string());
            argv.push(session.to_string());
        }
    }
    argv
}

/// Fresh (non-resume) or review: `codex exec --json --skip-git-repo-check -C
/// <worktree> --sandbox workspace-write -c
/// sandbox_workspace_write.network_access=<bool> [--model] [-c
/// model_reasoning_effort="<e>"] -`; review uses `--sandbox read-only` and
/// drops the network flag (nothing to write, nothing to reach).
///
/// Resume is a different shape entirely -- `codex exec resume <id> --json
/// --skip-git-repo-check -c sandbox_mode="workspace-write" -c
/// sandbox_workspace_write.network_access=<bool> [-m model] [-c
/// model_reasoning_effort=...] -`, with no `-C`/`--sandbox` at all (cwd is
/// set at the process level instead; `resume` doesn't accept them).
pub fn codex_argv(req: &RunRequest) -> Vec<String> {
    if let Some(session) = req.resume {
        if !req.review {
            let mut argv = vec![
                "exec".to_string(),
                "resume".to_string(),
                session.to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
                "-c".to_string(),
                "sandbox_mode=\"workspace-write\"".to_string(),
                "-c".to_string(),
                format!(
                    "sandbox_workspace_write.network_access={}",
                    req.network_allowed
                ),
            ];
            if let Some((name, server)) = req.codex_mcp {
                argv.extend(codex_mcp_flags(name, server));
            }
            if let Some(model) = req.model {
                argv.push("-m".to_string());
                argv.push(model.to_string());
            }
            if let Some(effort) = req.effort {
                argv.push("-c".to_string());
                argv.push(format!("model_reasoning_effort=\"{effort}\""));
            }
            argv.push("-".to_string());
            return argv;
        }
    }

    let mut argv = vec!["exec".to_string()];
    // `--image=` right after `exec`: the flag takes several values, so it
    // must not sit where it could swallow the trailing `-`.
    for image in req.images {
        argv.push(format!("--image={}", image.to_string_lossy()));
    }
    argv.push("--json".to_string());
    argv.push("--skip-git-repo-check".to_string());
    argv.push("-C".to_string());
    argv.push(req.worktree.to_string_lossy().to_string());
    if req.review {
        argv.push("--sandbox".to_string());
        argv.push("read-only".to_string());
    } else {
        argv.push("--sandbox".to_string());
        argv.push("workspace-write".to_string());
        argv.push("-c".to_string());
        argv.push(format!(
            "sandbox_workspace_write.network_access={}",
            req.network_allowed
        ));
    }
    if let Some((name, server)) = req.codex_mcp {
        argv.extend(codex_mcp_flags(name, server));
    }
    if let Some(model) = req.model {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    if let Some(effort) = req.effort {
        argv.push("-c".to_string());
        argv.push(format!("model_reasoning_effort=\"{effort}\""));
    }
    argv.push("-".to_string());
    argv
}

pub fn build_argv(req: &RunRequest) -> Vec<String> {
    match req.harness {
        Harness::Claude => claude_argv(req),
        Harness::Codex => codex_argv(req),
    }
}

/// The Stop hook wiring for a Claude run: `orchd`'s own path, the daemon's
/// socket, and the token that maps this run's `hook.stop` calls back to its
/// task/attempt. `lean_output` wires `orchd hook rtk` to PreToolUse and sets
/// `bashOutputMaxChars` -- it needs no socket or token, since the hook never
/// contacts the daemon.
pub struct StopHook<'a> {
    pub orchd_path: &'a str,
    pub socket_path: &'a str,
    pub token: &'a str,
    pub lean_output: bool,
}

/// A few seconds' margin over the rtk subprocess's own 2s timeout
/// (`hook::RTK_REWRITE_TIMEOUT`) so Claude Code's own hook-level timeout
/// never fires first and the hook always gets to answer `{}` itself.
const RTK_HOOK_TIMEOUT_SECS: u64 = 5;

/// `Bash` prefixes an agent may never run directly (`permissions.deny`
/// below) and `orchd hook rtk` must never rewrite into an allow answer
/// either: a rewrite like `rtk git commit ...` would otherwise escape these
/// same deny rules through the hook's own `permissionDecision: allow`.
pub const DENIED_BASH_COMMANDS: [&str; 2] = ["git commit", "git push"];

/// `bashOutputMaxChars` under `variant.lean_output` (Claude Code's inline
/// Bash limit; `BASH_MAX_OUTPUT_LENGTH` only sizes the read-back of an
/// output saved to a file, so it would cap nothing): bigger than the
/// 2000-char failure tail orchd itself feeds back in a Stop-hook block
/// (`hook::TAIL_CHARS`), so a failing command's own reported output still
/// fits, and well below Claude Code's 30000-char default, so it actually
/// shrinks what a noisy command bills. Output past it is saved to a file
/// the agent can read.
pub const BASH_OUTPUT_MAX_CHARS: u32 = 10_000;

/// The `settings.json` written alongside a Claude run: profile
/// env/apiKeyHelper (opaque, passed through) + sandbox block (omitted for
/// `host`) + the Stop hook wired to `orchd hook stop` (omitted when
/// `stop_hook` is `None` -- a review session has no registered token, so
/// installing a hook for it would just be a guaranteed no-op fail-open
/// round trip).
pub fn build_claude_settings(
    profile: Option<&serde_json::Value>,
    sandbox: SandboxMode,
    allowed_domains: &[String],
    deny_read: &[String],
    stop_hook: Option<StopHook>,
) -> serde_json::Value {
    let mut obj = match profile {
        Some(serde_json::Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };

    if sandbox == SandboxMode::Native {
        obj.insert(
            "sandbox".to_string(),
            serde_json::json!({
                "enabled": true,
                // Headless runs answer no prompts (`--permission-prompts
                // none`), so without this every sandboxed command is denied.
                "autoAllowBashIfSandboxed": true,
                "allowUnsandboxedCommands": false,
                // Only `allowedDomains`: with `strictAllowlist` /
                // `allowUnixSockets` present, Claude Code denied every Bash
                // call headlessly (`cargo --version` included), so agents
                // could never build or test. `["*"]` opens the network.
                // Unix sockets stay blocked by the sandbox's own default
                // (checked: a connect from inside fails with EPERM).
                "network": {
                    "allowedDomains": allowed_domains,
                },
                // Keeps the agent from reading `control.token` / per-run key
                // files even if it somehow finds the data dir's path.
                "filesystem": {
                    "denyRead": deny_read,
                },
            }),
        );
    }

    // The daemon commits after the gates pass and nothing is ever pushed, so
    // the agent may not do either. Bash is allowed outright: a headless run
    // has no one to approve a prompt, and `autoAllowBashIfSandboxed` still
    // prompts for compound commands (`npm test; echo $?`), which then fail.
    // The sandbox, not the prompt, is the boundary.
    obj.insert(
        "permissions".to_string(),
        serde_json::json!({
            "allow": ["Bash"],
            "deny": DENIED_BASH_COMMANDS
                .iter()
                .map(|c| format!("Bash({c}:*)"))
                .collect::<Vec<_>>(),
        }),
    );

    if let Some(hook) = stop_hook {
        let command = |kind: &str| {
            format!(
                "{} hook {kind} --socket {} --token {}",
                shell_quote(hook.orchd_path),
                shell_quote(hook.socket_path),
                shell_quote(hook.token)
            )
        };
        let mut hooks = serde_json::json!({
            "Stop": [
                {
                    "hooks": [
                        {"type": "command", "command": command("stop"), "timeout": 600}
                    ]
                }
            ]
        });
        if hook.lean_output {
            let rtk_command = format!("{} hook rtk", shell_quote(hook.orchd_path));
            hooks["PreToolUse"] = serde_json::json!([{
                "matcher": "Bash",
                "hooks": [
                    {"type": "command", "command": rtk_command, "timeout": RTK_HOOK_TIMEOUT_SECS}
                ]
            }]);
            obj.insert(
                "bashOutputMaxChars".to_string(),
                serde_json::json!(BASH_OUTPUT_MAX_CHARS),
            );
        }
        obj.insert("hooks".to_string(), hooks);
    }

    serde_json::Value::Object(obj)
}

/// POSIX single-quote quoting: the hook command (and a profile's
/// `apiKeyHelper`) runs through a shell, and the real data dir
/// (`Application Support`) contains a space.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// -- stream event parsing --------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub usage_input: u64,
    pub usage_output: u64,
    pub usage_cached: u64,
    pub final_text: Option<String>,
    /// A harness-reported failure (Claude `is_error` result, Codex
    /// `turn.failed`/`error`), used as the failure detail when the run
    /// produced nothing.
    pub error: Option<String>,
    /// The run printed nothing for its stall timeout and was killed.
    pub stalled: bool,
    /// Claude only: input + cache creation + cache read tokens of the first
    /// `assistant` event, i.e. the whole prompt the first turn was sent,
    /// whether or not an earlier run left it in the prompt cache.
    pub first_turn_tokens: Option<u64>,
    /// Claude only: the usage of each top-level assistant message, by
    /// message id -- the stream repeats a message once per content block.
    pub messages: BTreeMap<String, MessageUsage>,
    /// `cost_usd` was priced from `messages`: the run printed no `result`.
    pub cost_estimated: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageUsage {
    pub model: String,
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl RunOutcome {
    /// A Claude run that ended without its `result` event (killed, stalled,
    /// crashed): price the messages it did stream, fill in their token
    /// usage, and mark the cost estimated. No-op once a cost is known, or
    /// when no message has a price.
    pub fn estimate_cost(&mut self, prices: &BTreeMap<String, Price>) {
        if self.cost_usd.is_some() {
            return;
        }
        let priced: Vec<(&MessageUsage, Price)> = self
            .messages
            .values()
            .filter_map(|m| price_for(prices, &m.model).map(|p| (m, p)))
            .collect();
        if priced.is_empty() {
            return;
        }
        self.cost_usd = Some(
            priced
                .iter()
                .map(|(m, p)| p.claude_cost(m.input, m.cache_write, m.cache_read, m.output))
                .sum(),
        );
        self.cost_estimated = true;
        let sum = |f: fn(&MessageUsage) -> u64| self.messages.values().map(f).sum::<u64>();
        self.usage_input = sum(|m| m.input);
        self.usage_cached = sum(|m| m.cache_read);
        self.usage_output = sum(|m| m.output);
    }
}

/// Folds a whole saved `events.jsonl` (stderr lines included; they are
/// skipped) into an outcome, as if streamed.
pub fn replay_events(harness: Harness, text: &str) -> RunOutcome {
    let mut outcome = RunOutcome::default();
    for line in text.lines() {
        feed_stream_line(harness, line, &mut outcome);
    }
    outcome
}

/// Folds one line of a harness's streamed JSON output into `outcome`,
/// returning a short human-readable note for the `log` subscribe event when
/// the line is worth surfacing (at most one line per event, per spec).
/// Unparseable or irrelevant lines are ignored, never fatal -- a harness
/// version bump changing an unrelated field must not break the run.
pub fn feed_stream_line(harness: Harness, line: &str, outcome: &mut RunOutcome) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    match harness {
        Harness::Claude => feed_claude_line(&v, outcome),
        Harness::Codex => feed_codex_line(&v, outcome),
    }
}

fn feed_claude_line(v: &serde_json::Value, outcome: &mut RunOutcome) -> Option<String> {
    let ty = v.get("type").and_then(|x| x.as_str())?;
    match ty {
        "system" if v.get("subtype").and_then(|x| x.as_str()) == Some("init") => {
            outcome.session_id = v
                .get("session_id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            Some("session started".to_string())
        }
        "result" => {
            outcome.cost_usd = v.get("total_cost_usd").and_then(|x| x.as_f64());
            if let Some(usage) = v.get("usage") {
                outcome.usage_input = usage
                    .get("input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                outcome.usage_output = usage
                    .get("output_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                outcome.usage_cached = usage
                    .get("cache_read_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
            }
            outcome.final_text = v
                .get("result")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            if v.get("is_error").and_then(|x| x.as_bool()) == Some(true) {
                outcome.error = outcome
                    .final_text
                    .clone()
                    .or(Some("Claude reported an error.".into()));
            }
            Some("session finished".to_string())
        }
        "assistant" => {
            if outcome.first_turn_tokens.is_none() {
                outcome.first_turn_tokens = first_turn_tokens(v);
            }
            record_message_usage(v, outcome);
            claude_tool_note(v)
        }
        _ => None,
    }
}

fn first_turn_tokens(v: &serde_json::Value) -> Option<u64> {
    let usage = v.get("message")?.get("usage")?;
    let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    Some(n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"))
}

/// A subagent's messages (`parent_tool_use_id` set) are left out: only the
/// top-level conversation is counted. A repeated message id keeps its latest
/// usage, which is the fullest.
fn record_message_usage(v: &serde_json::Value, outcome: &mut RunOutcome) {
    if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
        return;
    }
    let Some(message) = v.get("message") else {
        return;
    };
    let Some(usage) = message.get("usage") else {
        return;
    };
    let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let id = message
        .get("id")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", outcome.messages.len()));
    let model = message
        .get("model")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    outcome.messages.insert(
        id,
        MessageUsage {
            model,
            input: n("input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cache_read: n("cache_read_input_tokens"),
            output: n("output_tokens"),
        },
    );
}

fn claude_tool_note(v: &serde_json::Value) -> Option<String> {
    let blocks = v.get("message")?.get("content")?.as_array()?;
    blocks.iter().find_map(|b| {
        if b.get("type").and_then(|x| x.as_str()) != Some("tool_use") {
            return None;
        }
        let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
        let input = b.get("input");
        let detail = input
            .and_then(|i| {
                i.get("command")
                    .or_else(|| i.get("file_path"))
                    .or_else(|| i.get("pattern"))
            })
            .and_then(|x| x.as_str())
            .unwrap_or("");
        Some(short_note(&format!("{name} {detail}")))
    })
}

fn short_note(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line.to_string()
    }
}

fn feed_codex_line(v: &serde_json::Value, outcome: &mut RunOutcome) -> Option<String> {
    let ty = v.get("type").and_then(|x| x.as_str())?;
    match ty {
        "thread.started" => {
            outcome.session_id = v
                .get("thread_id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            Some("session started".to_string())
        }
        "turn.completed" => {
            if let Some(usage) = v.get("usage") {
                let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                outcome.usage_input = n("input_tokens");
                outcome.usage_output = n("output_tokens");
                outcome.usage_cached = n("cached_input_tokens");
            }
            Some("turn completed".to_string())
        }
        "turn.failed" | "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .or_else(|| v.get("message"))
                .map(|m| {
                    m.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| m.to_string())
                })
                .unwrap_or_else(|| "Codex reported an error.".into());
            outcome.error = Some(message.clone());
            Some(short_note(&message))
        }
        "item.completed" | "item.started" => {
            let item = v.get("item")?;
            let done = ty == "item.completed";
            match item.get("type").and_then(|x| x.as_str())? {
                "agent_message" if done => {
                    outcome.final_text = item
                        .get("text")
                        .and_then(|x| x.as_str())
                        .map(str::to_string);
                    Some("agent message".to_string())
                }
                "command_execution" => {
                    let cmd = item
                        .get("command")
                        .and_then(|x| x.as_str())
                        .unwrap_or("a command");
                    Some(short_note(&format!(
                        "{} {cmd}",
                        if done { "Ran" } else { "Running" }
                    )))
                }
                "file_change" if done => Some("Edited files".to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn base_req(harness: Harness, worktree: &Path) -> RunRequest<'_> {
        RunRequest {
            harness,
            worktree,
            model: None,
            effort: None,
            resume: None,
            review: false,
            mcp_config: None,
            settings_path: None,
            network_allowed: true,
            codex_mcp: None,
            images: &[],
            repo_settings: true,
        }
    }

    #[test]
    fn codex_gets_the_messages_server_fresh_and_resumed() {
        let wt = PathBuf::from("/repo-task");
        let server = serde_json::json!({"command": "/o", "args": ["mcp", "--task", "t"]});
        let mut req = base_req(Harness::Codex, &wt);
        req.codex_mcp = Some(("sushiai-messages", &server));
        let command = "mcp_servers.sushiai-messages.command=\"/o\"".to_string();
        let args = "mcp_servers.sushiai-messages.args=[\"mcp\",\"--task\",\"t\"]".to_string();
        let fresh = codex_argv(&req);
        assert!(fresh.contains(&command));
        assert!(fresh.contains(&args));
        req.resume = Some("thread-1");
        let resumed = codex_argv(&req);
        assert!(resumed.contains(&command));
        assert_eq!(resumed.last().unwrap(), "-");
    }

    #[test]
    fn claude_implement_argv_has_accept_edits_and_no_tools_restriction() {
        let wt = PathBuf::from("/repo-task");
        let mcp = PathBuf::from("/repo-task/runs/1/mcp.json");
        let settings = PathBuf::from("/repo-task/runs/1/settings.json");
        let mut req = base_req(Harness::Claude, &wt);
        req.mcp_config = Some(&mcp);
        req.settings_path = Some(&settings);
        req.model = Some("sonnet");
        let argv = claude_argv(&req);
        assert!(argv.contains(&"--permission-mode".to_string()));
        let idx = argv.iter().position(|a| a == "--permission-mode").unwrap();
        assert_eq!(argv[idx + 1], "acceptEdits");
        assert!(!argv.contains(&"--tools".to_string()));
        assert!(argv.contains(&"--mcp-config".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"--resume".to_string()));
    }

    /// Pins today's argv: an implement run always trims the delegation
    /// tools now, unconditionally.
    #[test]
    fn claude_implement_argv_always_trims_delegation_tools() {
        let wt = PathBuf::from("/w");
        let mcp = PathBuf::from("/r/mcp.json");
        let settings = PathBuf::from("/r/settings.json");
        let mut req = base_req(Harness::Claude, &wt);
        req.mcp_config = Some(&mcp);
        req.settings_path = Some(&settings);
        req.model = Some("opus");
        req.effort = Some("high");
        req.resume = Some("sess-1");
        let expected: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--setting-sources",
            "project,local",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--mcp-config",
            "/r/mcp.json",
            "--settings",
            "/r/settings.json",
            "--permission-mode",
            "acceptEdits",
            "--disallowedTools",
            DELEGATION_TOOLS,
            "--permission-prompts",
            "none",
            "--model",
            "opus",
            "--effort",
            "high",
            "--resume",
            "sess-1",
        ]
        .map(String::from)
        .to_vec();
        let argv = claude_argv(&req);
        assert_eq!(argv, expected);
        let at = argv.iter().position(|a| a == "--disallowedTools").unwrap();
        assert!(argv[at + 1].split(',').any(|t| t == "Task"));
    }

    #[test]
    fn claude_review_never_gets_the_delegation_tools_trim() {
        let wt = PathBuf::from("/w");
        let mut req = base_req(Harness::Claude, &wt);
        req.review = true;
        assert!(!claude_argv(&req).contains(&"--disallowedTools".to_string()));
    }

    #[test]
    fn claude_resume_appends_resume_flag() {
        let wt = PathBuf::from("/repo-task");
        let mut req = base_req(Harness::Claude, &wt);
        req.resume = Some("sess-123");
        let argv = claude_argv(&req);
        let idx = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(argv[idx + 1], "sess-123");
    }

    #[test]
    fn claude_review_uses_read_only_tools_and_plan_mode_and_never_resumes() {
        let wt = PathBuf::from("/repo-task");
        let mut req = base_req(Harness::Claude, &wt);
        req.review = true;
        req.resume = Some("sess-123"); // must be ignored in review mode
        let argv = claude_argv(&req);
        assert!(argv.contains(&"--tools".to_string()));
        let tools_idx = argv.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(argv[tools_idx + 1], "Read,Grep,Glob");
        let mode_idx = argv.iter().position(|a| a == "--permission-mode").unwrap();
        assert_eq!(argv[mode_idx + 1], "plan");
        assert!(!argv.contains(&"--resume".to_string()));
    }

    #[test]
    fn codex_implement_argv_sets_workspace_write_and_network_flag() {
        let wt = PathBuf::from("/repo-task");
        let mut req = base_req(Harness::Codex, &wt);
        req.network_allowed = false;
        let argv = codex_argv(&req);
        assert!(argv.contains(&"workspace-write".to_string()));
        assert!(argv.contains(&"sandbox_workspace_write.network_access=false".to_string()));
        assert_eq!(argv.last().unwrap(), "-");
    }

    #[test]
    fn codex_review_attaches_images_before_the_other_flags() {
        let images = vec![std::path::PathBuf::from("/w/artifacts/a.png")];
        let mut req = base_req(Harness::Codex, Path::new("/w"));
        req.review = true;
        req.images = &images;
        let argv = codex_argv(&req);
        assert_eq!(argv[1], "--image=/w/artifacts/a.png");
        assert_eq!(argv.last().unwrap(), "-");
    }

    #[test]
    fn codex_resume_uses_resume_subcommand_with_no_dash_c_flag_or_sandbox_flag() {
        let wt = PathBuf::from("/repo-task");
        let mut req = base_req(Harness::Codex, &wt);
        req.resume = Some("thread-1");
        req.model = Some("gpt-5-codex");
        req.effort = Some("high");
        req.network_allowed = true;
        let argv = codex_argv(&req);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "resume");
        assert_eq!(argv[2], "thread-1");
        // `codex exec resume` doesn't accept `-C`/`--sandbox` at all (cwd is
        // set at the process level, sandbox mode goes through `-c` instead).
        assert!(!argv.contains(&"-C".to_string()));
        assert!(!argv.contains(&"--sandbox".to_string()));
        assert!(argv.contains(&"sandbox_mode=\"workspace-write\"".to_string()));
        assert!(argv.contains(&"sandbox_workspace_write.network_access=true".to_string()));
        // resume takes the model via `-m`, not `--model`.
        assert!(!argv.contains(&"--model".to_string()));
        let m_idx = argv.iter().position(|a| a == "-m").unwrap();
        assert_eq!(argv[m_idx + 1], "gpt-5-codex");
        assert_eq!(argv.last().unwrap(), "-");
    }

    #[test]
    fn codex_review_is_sandboxed_read_only() {
        let wt = PathBuf::from("/repo-task");
        let mut req = base_req(Harness::Codex, &wt);
        req.review = true;
        let argv = codex_argv(&req);
        assert!(argv.contains(&"read-only".to_string()));
        assert!(!argv.iter().any(|a| a.contains("network_access")));
    }

    #[test]
    fn claude_settings_merges_profile_and_omits_sandbox_for_host() {
        let profile = serde_json::json!({"env": {"ANTHROPIC_API_KEY_HELPER": "x"}, "apiKeyHelper": "helper.sh"});
        let domains = vec!["github.com".to_string()];
        let hook = StopHook {
            orchd_path: "/usr/local/bin/orchd",
            socket_path: "/tmp/orchd.sock",
            token: "tok-1",
            lean_output: false,
        };
        let deny_read = vec!["/data".to_string()];
        let native = build_claude_settings(
            Some(&profile),
            SandboxMode::Native,
            &domains,
            &deny_read,
            Some(hook),
        );
        assert_eq!(native["apiKeyHelper"], "helper.sh");
        assert_eq!(native["sandbox"]["enabled"], true);
        assert_eq!(
            native["sandbox"]["network"]["allowedDomains"][0],
            "github.com"
        );
        assert!(native["sandbox"]["network"]
            .get("strictAllowlist")
            .is_none());
        assert_eq!(native["sandbox"]["filesystem"]["denyRead"][0], "/data");
        assert_eq!(native["hooks"]["Stop"][0]["hooks"][0]["timeout"], 600);
        assert!(native["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hook stop --socket '/tmp/orchd.sock' --token 'tok-1'"));

        let hook2 = StopHook {
            orchd_path: "/usr/local/bin/orchd",
            socket_path: "/tmp/orchd.sock",
            token: "tok-1",
            lean_output: false,
        };
        let host =
            build_claude_settings(None, SandboxMode::Host, &domains, &deny_read, Some(hook2));
        assert!(host.get("sandbox").is_none());
        assert!(host.get("hooks").is_some());
    }

    #[test]
    fn claude_settings_wire_the_rtk_hook_and_output_cap_only_when_asked() {
        let hook = |lean_output| StopHook {
            orchd_path: "/bin/orchd",
            socket_path: "/tmp/o.sock",
            token: "tok",
            lean_output,
        };
        let profile = serde_json::json!({"env": {"ANTHROPIC_API_KEY_HELPER": "x"}});
        let off = build_claude_settings(
            Some(&profile),
            SandboxMode::Host,
            &[],
            &[],
            Some(hook(false)),
        );
        let keys: Vec<&String> = off["hooks"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["Stop"], "the Stop hook only: {off}");
        assert!(off.get("bashOutputMaxChars").is_none());
        assert_eq!(off["env"]["ANTHROPIC_API_KEY_HELPER"], "x");

        let on = build_claude_settings(
            Some(&profile),
            SandboxMode::Host,
            &[],
            &[],
            Some(hook(true)),
        );
        assert_eq!(on["hooks"]["Stop"], off["hooks"]["Stop"]);
        let pre = &on["hooks"]["PreToolUse"][0];
        assert_eq!(pre["matcher"], "Bash");
        let rtk = &pre["hooks"][0];
        assert_eq!(rtk["command"], "'/bin/orchd' hook rtk");
        assert!(rtk["timeout"].as_u64().unwrap() > 0);
        // A profile's own env keys stay.
        assert_eq!(on["env"]["ANTHROPIC_API_KEY_HELPER"], "x");
        assert_eq!(on["bashOutputMaxChars"], BASH_OUTPUT_MAX_CHARS);
    }

    #[test]
    fn claude_settings_omits_hooks_entirely_when_no_stop_hook_given() {
        let domains = vec!["github.com".to_string()];
        let review = build_claude_settings(None, SandboxMode::Native, &domains, &[], None);
        assert!(
            review.get("hooks").is_none(),
            "a review session has no registered token, so it must not get a Stop hook"
        );
        assert_eq!(review["sandbox"]["enabled"], true);
    }

    #[test]
    fn feeds_claude_stream_lines_into_outcome() {
        let mut outcome = RunOutcome::default();
        feed_stream_line(
            Harness::Claude,
            r#"{"type":"system","subtype":"init","session_id":"sess-9"}"#,
            &mut outcome,
        );
        feed_stream_line(
            Harness::Claude,
            r#"{"type":"result","total_cost_usd":0.42,"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5},"result":"done"}"#,
            &mut outcome,
        );
        assert_eq!(outcome.session_id.as_deref(), Some("sess-9"));
        assert_eq!(outcome.cost_usd, Some(0.42));
        assert_eq!(outcome.usage_input, 10);
        assert_eq!(outcome.final_text.as_deref(), Some("done"));
    }

    #[test]
    fn feeds_codex_stream_lines_into_outcome() {
        let mut outcome = RunOutcome::default();
        feed_stream_line(
            Harness::Codex,
            r#"{"type":"thread.started","thread_id":"th-1"}"#,
            &mut outcome,
        );
        feed_stream_line(
            Harness::Codex,
            r#"{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":4,"cached_input_tokens":1}}"#,
            &mut outcome,
        );
        feed_stream_line(
            Harness::Codex,
            r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"all set"}}"#,
            &mut outcome,
        );
        assert_eq!(outcome.session_id.as_deref(), Some("th-1"));
        assert_eq!(outcome.usage_output, 4);
        assert_eq!(outcome.usage_cached, 1);
        assert!(outcome.error.is_none());
        feed_stream_line(
            Harness::Codex,
            r#"{"type":"turn.failed","error":{"message":"model not supported"}}"#,
            &mut outcome,
        );
        assert_eq!(outcome.error.as_deref(), Some("model not supported"));
        assert_eq!(outcome.final_text.as_deref(), Some("all set"));
    }

    /// Shape captured from `claude -p --output-format stream-json --verbose`
    /// (2.1.283): the first turn read most of its prompt from the cache.
    #[test]
    fn first_turn_tokens_count_cached_prompt_tokens_from_the_first_assistant_event() {
        let mut outcome = RunOutcome::default();
        for line in [
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":9,"cache_creation_input_tokens":11644,"cache_read_input_tokens":18198,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":11644},"output_tokens":4,"service_tier":"standard"}},"session_id":"s"}"#,
            r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":5,"cache_read_input_tokens":40000}},"session_id":"s"}"#,
            r#"{"type":"result","usage":{"input_tokens":14,"cache_read_input_tokens":58198,"output_tokens":9},"result":"ok"}"#,
        ] {
            feed_stream_line(Harness::Claude, line, &mut outcome);
        }
        assert_eq!(outcome.first_turn_tokens, Some(9 + 11644 + 18198));

        let mut none = RunOutcome::default();
        feed_stream_line(
            Harness::Claude,
            r#"{"type":"result","usage":{"input_tokens":1},"result":"ok"}"#,
            &mut none,
        );
        assert_eq!(none.first_turn_tokens, None);
    }

    /// Shape of `claude -p --output-format stream-json --verbose` (2.1.283)
    /// cut off before its `result`: one message streamed as two events (a
    /// text block, then a tool_use block) under the same id, a second
    /// message, a subagent's message, and a stderr line from events.jsonl.
    #[test]
    fn a_run_without_a_result_is_priced_from_its_unique_messages_and_marked_estimated() {
        let events = [
            r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5-5"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_01A","type":"message","role":"assistant","content":[{"type":"text","text":"Reading."}],"usage":{"input_tokens":6,"cache_creation_input_tokens":12000,"cache_read_input_tokens":18000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":12000},"output_tokens":30,"service_tier":"standard"}},"parent_tool_use_id":null,"session_id":"s"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_01A","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/w/a.rs"}}],"usage":{"input_tokens":6,"cache_creation_input_tokens":12000,"cache_read_input_tokens":18000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":12000},"output_tokens":30,"service_tier":"standard"}},"parent_tool_use_id":null,"session_id":"s"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"fn a() {}"}]},"parent_tool_use_id":null,"session_id":"s"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_01B","type":"message","role":"assistant","content":[{"type":"text","text":"Editing."}],"usage":{"input_tokens":2,"cache_creation_input_tokens":500,"cache_read_input_tokens":30000,"output_tokens":400,"service_tier":"standard"}},"parent_tool_use_id":null,"session_id":"s"}"#,
            r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_sub","type":"message","role":"assistant","content":[],"usage":{"input_tokens":9999,"output_tokens":9999}},"parent_tool_use_id":"toolu_9","session_id":"s"}"#,
            "[stderr] warning: something",
        ]
        .join("\n");
        let mut outcome = replay_events(Harness::Claude, &events);
        assert_eq!(outcome.messages.len(), 2);
        assert_eq!(outcome.cost_usd, None);
        let prices = crate::model::Settings::default().prices;
        outcome.estimate_cost(&prices);
        let expected = (6.0 * 4.0 + 12_000.0 * 5.0 + 18_000.0 * 0.20 + 30.0 * 20.0) / 1e6
            + (2.0 * 4.0 + 500.0 * 5.0 + 30_000.0 * 0.20 + 400.0 * 20.0) / 1e6;
        let cost = outcome.cost_usd.unwrap();
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");
        assert!(outcome.cost_estimated);
        assert_eq!(
            (
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output
            ),
            (8, 48_000, 430)
        );

        // A run that did print its result keeps the CLI's own figure.
        let finished = format!(
            "{events}\n{}",
            r#"{"type":"result","total_cost_usd":0.5,"usage":{"input_tokens":8},"result":"ok"}"#
        );
        let mut outcome = replay_events(Harness::Claude, &finished);
        outcome.estimate_cost(&prices);
        assert_eq!(outcome.cost_usd, Some(0.5));
        assert!(!outcome.cost_estimated);

        // No price for the model: no guess.
        let mut unpriced = replay_events(Harness::Claude, &events.replace("claude-opus-5-5", "x"));
        unpriced.estimate_cost(&prices);
        assert_eq!(unpriced.cost_usd, None);
        assert!(!unpriced.cost_estimated);
    }

    #[test]
    fn malformed_lines_are_ignored_not_fatal() {
        let mut outcome = RunOutcome::default();
        let note = feed_stream_line(Harness::Claude, "not json at all", &mut outcome);
        assert!(note.is_none());
        assert!(outcome.session_id.is_none());
    }

    /// Lines captured from real `codex exec --json` (0.154) and
    /// `claude -p --output-format stream-json` (2.1.280) runs, trimmed to the
    /// fields we read. A hand-written fake once agreed with a wrong parser;
    /// these keep the parser honest against the CLIs' actual shapes.
    #[test]
    fn parses_captured_real_cli_streams() {
        let mut codex = RunOutcome::default();
        for line in [
            r#"{"type":"thread.started","thread_id":"01a0ceb4-8341-7a52-98b6-e8a84e9db9cf"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"`[features].collab` is deprecated."}}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"hi"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":19775,"cached_input_tokens":12928,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}"#,
        ] {
            feed_stream_line(Harness::Codex, line, &mut codex);
        }
        assert_eq!(
            codex.session_id.as_deref(),
            Some("01a0ceb4-8341-7a52-98b6-e8a84e9db9cf")
        );
        assert_eq!(codex.final_text.as_deref(), Some("hi"));
        assert_eq!(
            (codex.usage_input, codex.usage_cached, codex.usage_output),
            (19775, 12928, 5)
        );
        assert!(
            codex.error.is_none(),
            "a deprecation item is not a run failure"
        );

        let mut claude = RunOutcome::default();
        for line in [
            r#"{"type":"system","subtype":"init","session_id":"6e8e54b3-2bcb-48aa-9b85-5f9a848efe52","model":"claude-haiku-4-5-20251001"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"hi","session_id":"6e8e54b3-2bcb-48aa-9b85-5f9a848efe52","total_cost_usd":0.0133868,"usage":{"input_tokens":9,"cache_read_input_tokens":18198,"output_tokens":50}}"#,
        ] {
            feed_stream_line(Harness::Claude, line, &mut claude);
        }
        assert_eq!(
            claude.session_id.as_deref(),
            Some("6e8e54b3-2bcb-48aa-9b85-5f9a848efe52")
        );
        assert_eq!(claude.final_text.as_deref(), Some("hi"));
        assert_eq!(claude.cost_usd, Some(0.0133868));
        assert_eq!(
            (claude.usage_input, claude.usage_cached, claude.usage_output),
            (9, 18198, 50)
        );
        assert!(claude.error.is_none());
    }

    #[test]
    fn claude_settings_allow_sandboxed_bash_and_forbid_commit_and_push() {
        let native = build_claude_settings(None, SandboxMode::Native, &[], &[], None);
        assert_eq!(native["sandbox"]["autoAllowBashIfSandboxed"], true);
        let deny = native["permissions"]["deny"].as_array().unwrap();
        assert!(deny.iter().any(|d| d == "Bash(git commit:*)"));
        assert!(deny.iter().any(|d| d == "Bash(git push:*)"));
        assert_eq!(native["permissions"]["allow"], serde_json::json!(["Bash"]));

        let host = build_claude_settings(None, SandboxMode::Host, &[], &[], None);
        assert_eq!(host["permissions"]["allow"], serde_json::json!(["Bash"]));
    }
}
