use super::outcome::RunOutcome;
use super::request::RunRequest;
use super::stream::short_note;

/// `-c mcp_servers.<name>.{command,args,env.*}` for one MCP server, or
/// `.url` for an HTTP one. JSON strings and string arrays are valid TOML
/// values as they are. A definition Codex cannot take as it is -- a name
/// that is not a bare TOML key, no command or url, an HTTP server with
/// headers (they would be dropped, so it would connect without its auth)
/// or SSE -- gets no flags at all.
pub fn codex_mcp_flags(name: &str, server: &serde_json::Value) -> Vec<String> {
    let bare = |k: &str| {
        !k.is_empty()
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if !bare(name) {
        return vec![];
    }
    if let Some(url) = server.get("url").filter(|u| u.is_string()) {
        if server.get("headers").is_some() || server["type"] == "sse" {
            return vec![];
        }
        return vec!["-c".to_string(), format!("mcp_servers.{name}.url={url}")];
    }
    let Some(command) = server.get("command").filter(|c| c.is_string()) else {
        return vec![];
    };
    let mut flags = vec![
        "-c".to_string(),
        format!("mcp_servers.{name}.command={command}"),
    ];
    if let Some(args) = server.get("args").filter(|a| a.is_array()) {
        flags.push("-c".to_string());
        flags.push(format!("mcp_servers.{name}.args={args}"));
    }
    if let Some(env) = server.get("env").and_then(|e| e.as_object()) {
        for (key, value) in env.iter().filter(|(k, v)| bare(k) && v.is_string()) {
            flags.push("-c".to_string());
            flags.push(format!("mcp_servers.{name}.env.{key}={value}"));
        }
    }
    flags
}

/// `codex exec --json --skip-git-repo-check -C
/// <worktree> --sandbox danger-full-access` for task implement runs wrapped
/// by orchd; other non-review runs keep `workspace-write`. The config
/// sandbox_workspace_write.network_access=<bool> [--model] [-c
/// model_reasoning_effort="<e>"] -`; review uses `--sandbox read-only` and
/// drops the network flag (nothing to write, nothing to reach).
pub fn codex_argv(req: &RunRequest) -> Vec<String> {
    let mut argv = vec!["exec".to_string()];
    if let Some(session) = req.resume {
        argv.push("resume".to_string());
        argv.push(session.to_string());
    }
    // `--image=` right after `exec`: the flag takes several values, so it
    // must not sit where it could swallow the trailing `-`.
    for image in req.images {
        argv.push(format!("--image={}", image.to_string_lossy()));
    }
    argv.push("--json".to_string());
    argv.push("--skip-git-repo-check".to_string());
    let mode = if req.review {
        "read-only"
    } else if task_run_uses_outer_sandbox(req) {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    if req.resume.is_some() {
        // `resume` takes neither `-C` nor `--sandbox`; the cwd is set on the
        // process and the sandbox through config.
        argv.push("-c".to_string());
        argv.push(format!("sandbox_mode=\"{mode}\""));
    } else {
        argv.push("-C".to_string());
        argv.push(req.worktree.to_string_lossy().to_string());
        argv.push("--sandbox".to_string());
        argv.push(mode.to_string());
    }
    if !req.review {
        argv.push("-c".to_string());
        argv.push(format!(
            "sandbox_workspace_write.network_access={}",
            req.network_allowed
        ));
    }
    for (name, server) in req.codex_mcp {
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

/// Whether orchd will wrap this task run in its native macOS profile.
/// Codex can use full access only when that outer profile is actually present.
pub(crate) fn task_run_uses_outer_sandbox(req: &RunRequest<'_>) -> bool {
    cfg!(target_os = "macos") && req.task_run && !req.review && req.task_sandbox.is_some()
}

pub(super) fn feed_codex_line(v: &serde_json::Value, outcome: &mut RunOutcome) -> Option<String> {
    if let Some(model) = v.get("model").and_then(|x| x.as_str()) {
        outcome.init_model = Some(model.to_string());
    }
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
