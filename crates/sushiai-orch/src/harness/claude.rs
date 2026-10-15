use super::failure::ProviderFailure;
use super::outcome::{MessageUsage, PermissionDenial, RunOutcome};
use super::request::RunRequest;
use super::stream::short_note;
use crate::policy::settings::SandboxMode;
use crate::sandbox::TaskSandbox;

/// The tools an orchd agent never needs: it implements one bounded task and
/// does not delegate. `Task` and `Agent` are the subagent tool's two names.
pub const DELEGATION_TOOLS: &str = "Task,Agent,Workflow,SendMessage,ListAgents";

/// `claude -p --output-format stream-json --verbose --setting-sources
/// project,local --disable-slash-commands --strict-mcp-config --mcp-config
/// <mcp.json> --settings <settings.json> --permission-mode acceptEdits
/// --disallowedTools <DELEGATION_TOOLS> --permission-prompts none [--model]
/// [--effort] [--max-budget-usd] `; review swaps `--permission-mode acceptEdits`
/// (and the delegation-tools trim, which review never gets) for
/// `--tools Read,Grep,Glob --permission-mode plan`.
pub fn claude_argv(req: &RunRequest) -> Vec<String> {
    let mut argv = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--setting-sources".to_string(),
        // Project sandbox grants merge with CLI settings and can widen the policy.
        if req.repo_settings && !(req.task_run && req.task_sandbox.is_some()) {
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
    if let Some(cap) = req.max_budget_usd {
        argv.push("--max-budget-usd".to_string());
        argv.push(cap.to_string());
    }
    if let Some(session) = req.resume {
        argv.push("--resume".to_string());
        argv.push(session.to_string());
    }
    argv
}

/// The Stop hook wiring for a Claude run: the `sushiai` executable, the home
/// whose daemon it calls, and the token that maps this run's `hook.stop`
/// calls back to its task/attempt.
pub struct StopHook<'a> {
    pub exe_path: &'a str,
    pub home: &'a str,
    pub token: &'a str,
}

/// The tools `hook.edit` decides for: file edits (lease check, protected
/// paths), every MCP tool (a write asks the owner) and the web tools a
/// headless run is not allowed to use on its own.
pub const HOOKED_TOOLS: &str = "Edit|Write|MultiEdit|NotebookEdit|mcp__.*|WebFetch|WebSearch";

/// How long a hooked call may be held for an owner answer (seconds).
pub const HOOK_HOLD_SECS: u32 = 4 * 3600;

/// `Bash` prefixes an agent may never run directly (`permissions.deny`
/// below).
pub const DENIED_BASH_COMMANDS: [&str; 2] = ["git commit", "git push"];

/// The `settings.json` written alongside a Claude run: profile
/// env/apiKeyHelper (opaque, passed through) + sandbox block (omitted for
/// `host`) + the Stop hook wired to `sushiai orch hook stop` (omitted when
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
                // Keeps the agent from reading per-run key files even if it
                // somehow finds the data dir's path.
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
            // The token and home travel as environment assignments the hook's
            // shell understands, so the command line needs no flags.
            format!(
                "SUSHIAI_HOME={} SUSHIAI_ORCH_TOKEN={} {} orch hook {kind}",
                shell_quote(hook.home),
                shell_quote(hook.token),
                shell_quote(hook.exe_path),
            )
        };
        let hooks = serde_json::json!({
            "Stop": [
                {
                    "hooks": [
                        {"type": "command", "command": command("stop"), "timeout": 600}
                    ]
                }
            ],
            // Asks the daemon (`hook.edit`) whether another live task holds
            // the file, and holds a call the run may not make until the owner
            // answers; a denial names why so the agent goes on.
            "PreToolUse": [
                {
                    "matcher": HOOKED_TOOLS,
                    "hooks": [
                        {"type": "command", "command": command("edit"), "timeout": HOOK_HOLD_SECS}
                    ]
                }
            ]
        });
        obj.insert("hooks".to_string(), hooks);
    }

    serde_json::Value::Object(obj)
}

/// Implement settings share the task policy; other stages keep their old settings.
pub fn build_claude_task_settings(
    profile: Option<&serde_json::Value>,
    sandbox: Option<&TaskSandbox>,
    allowed_domains: &[String],
    stop_hook: Option<StopHook<'_>>,
) -> serde_json::Value {
    let denied: Vec<String> = sandbox
        .into_iter()
        .flat_map(|policy| &policy.denied_read_roots)
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let mode = if sandbox.is_some() {
        SandboxMode::Native
    } else {
        SandboxMode::Host
    };
    let mut settings = build_claude_settings(profile, mode, allowed_domains, &denied, stop_hook);
    if let Some(policy) = sandbox {
        settings["sandbox"]["filesystem"]["allowWrite"] = serde_json::json!(policy.write_roots);
        settings["sandbox"]["filesystem"]["allowRead"] = serde_json::json!(policy.read_exceptions);
        let deny = settings["permissions"]["deny"].as_array_mut().unwrap();
        deny.extend(
            policy
                .read_tool_denied_roots
                .iter()
                .map(|path| serde_json::json!(format!("Read(/{}/**)", path.display()))),
        );
    }
    settings
}

/// POSIX single-quote quoting: the hook command (and a profile's
/// `apiKeyHelper`) runs through a shell, and the real data dir
/// (`Application Support`) contains a space.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The failure class a Claude stream reports: the assistant message's `error`
/// category, or the result's HTTP status. `None` when neither names one, so
/// the text decides.
fn claude_failure(category: Option<&str>, status: Option<u64>) -> Option<ProviderFailure> {
    match (category, status) {
        (Some("rate_limit"), _) | (_, Some(429)) => Some(ProviderFailure::RateLimited {
            retry_after_ms: None,
        }),
        (Some("billing_error"), _) => Some(ProviderFailure::QuotaExhausted {
            retry_after_ms: None,
        }),
        (Some("authentication_failed"), _) | (_, Some(401)) => Some(ProviderFailure::Auth),
        (_, Some(529)) => Some(ProviderFailure::Overloaded {
            retry_after_ms: None,
        }),
        _ => None,
    }
}

pub(super) fn feed_claude_line(v: &serde_json::Value, outcome: &mut RunOutcome) -> Option<String> {
    let ty = v.get("type").and_then(|x| x.as_str())?;
    match ty {
        "system" if v.get("subtype").and_then(|x| x.as_str()) == Some("init") => {
            outcome.session_id = v
                .get("session_id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            outcome.init_model = v.get("model").and_then(|x| x.as_str()).map(str::to_string);
            outcome.harness_version = v
                .get("claude_code_version")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            Some("session started".to_string())
        }
        "result" => {
            if let Some(usage) = v.get("modelUsage").and_then(|x| x.as_object()) {
                for key in usage.keys() {
                    if !outcome.usage_models.contains(key) {
                        outcome.usage_models.push(key.clone());
                    }
                }
            }
            outcome.cost_usd = v.get("total_cost_usd").and_then(|x| x.as_f64());
            outcome.permission_denials = v
                .get("permission_denials")
                .and_then(|x| x.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|d| {
                            Some(PermissionDenial {
                                tool: d.get("tool_name")?.as_str()?.to_string(),
                                input: d.get("tool_input").cloned().unwrap_or_default(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
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
            // With `--json-schema` the checked reply is `structured_output`; the
            // result text is prose around it.
            if let Some(structured) = v.get("structured_output").filter(|s| !s.is_null()) {
                outcome.final_text = Some(structured.to_string());
            }
            if v.get("subtype").and_then(|x| x.as_str()) == Some("error_max_budget_usd") {
                outcome.over_budget = true;
            }
            if v.get("is_error").and_then(|x| x.as_bool()) == Some(true) {
                outcome.error = outcome
                    .final_text
                    .clone()
                    .or(Some("Claude reported an error.".into()));
                let status = v.get("api_error_status").and_then(|x| x.as_u64());
                if let Some(class) = claude_failure(None, status) {
                    outcome.structured_failure = Some(class);
                }
            }
            Some("session finished".to_string())
        }
        "assistant" => {
            let category = v.get("error").and_then(|x| x.as_str());
            if let Some(class) = claude_failure(category, None) {
                outcome.structured_failure = Some(class);
            }
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
