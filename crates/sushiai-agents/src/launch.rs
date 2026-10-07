//! Build the argv and the child variables that start an agent in a session.
//!
//! Claude gets exactly one `--settings` JSON holding the status hooks and the
//! permission approver (a second `--settings` flag is undocumented). Codex
//! gets no per-launch hooks: its entries live in `~/.codex/hooks.json` (see
//! `codex_hooks`) and stay inert without the session variables. Codex starts
//! with `--no-daemon`, so its hooks run in the child that has those variables.

use serde_json::{json, Map, Value};

use crate::Agent;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LaunchError {
    #[error("hook binary path must be absolute, got {0:?}")]
    RelativeHookPath(String),
    #[error("hook binary must be named `sushiai`, got {0:?}")]
    HookBinName(String),
    #[error("extra argument {0:?} would override what the daemon sets")]
    ForbiddenArg(String),
    #[error("invalid agent session id {0:?}")]
    BadSessionId(String),
    #[error("claudeSettings must not contain a `hooks` key: the daemon owns the hooks")]
    SettingsHooks,
    #[error("claudeSettings key {0:?} is not allowed")]
    SettingsKey(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSession<'a> {
    /// A new session. Claude is told this uuid (`--session-id`); Codex mints
    /// its own id and reports it through `SessionStart`.
    New(&'a str),
    Resume(&'a str),
}

#[derive(Debug, Clone)]
pub struct LaunchParams<'a> {
    pub agent: Agent,
    /// Absolute path of `~/.sushiai/bin/sushiai`, expanded by the caller.
    pub hook_bin: &'a str,
    pub socket: &'a str,
    pub sushiai_session_id: &'a str,
    pub session_token: &'a str,
    pub session: AgentSession<'a>,
    /// Flags passed through before the prompt. A flag that takes several
    /// values (`--mcp-config`) must not be last when a prompt follows.
    pub extra_args: &'a [String],
    pub prompt: Option<&'a str>,
    /// How long the approver may wait for the owner (spec default 600).
    pub permission_wait_secs: u64,
    /// Extra top-level keys for Claude's `--settings` (may hold secrets). Our `hooks` win.
    pub claude_settings: Option<&'a Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub argv: Vec<String>,
    /// Variables to set on the PTY child (the hook reads them).
    pub vars: Vec<(String, String)>,
}

/// Quote one word for `sh -c`. Safe words pass through unchanged.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./,:=@%+".contains(c));
    if safe {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The hook command line: `<bin> hook <event> [--agent codex]`.
pub fn hook_command(bin: &str, event: &str, agent: Agent) -> String {
    let base = format!("{} hook {}", shell_quote(bin), event);
    match agent {
        Agent::Claude => base,
        Agent::Codex => format!("{base} --agent codex"),
    }
}

/// Hook timeout that covers the approver's wait plus a margin. One owner for
/// Claude settings and the Codex entry.
pub fn permission_timeout(wait_secs: u64) -> u64 {
    wait_secs.saturating_add(30)
}

/// A uuid or a plain token (`[A-Za-z0-9._-]+`) that does not start with `-`.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The Codex flag (0.160 and later) that keeps the session in the `codex` process itself.
pub const CODEX_NO_DAEMON: &str = "--no-daemon";

fn check_extra_args(args: &[String]) -> Result<(), LaunchError> {
    for a in args {
        let name = a.split('=').next().unwrap_or(a);
        if matches!(
            name,
            "--settings" | "--bare" | "--session-id" | "--resume" | "-r"
        ) {
            return Err(LaunchError::ForbiddenArg(a.clone()));
        }
    }
    Ok(())
}

pub(crate) fn require_absolute(bin: &str) -> Result<(), LaunchError> {
    if bin.starts_with('/') {
        Ok(())
    } else {
        Err(LaunchError::RelativeHookPath(bin.to_owned()))
    }
}

/// (Claude event, hook argument, async). Status-only hooks run async so they
/// never slow the agent; the ones that answer or must finish first do not.
const CLAUDE_HOOKS: &[(&str, &str, bool)] = &[
    ("SessionStart", "session-start", false),
    ("UserPromptSubmit", "prompt", true),
    ("PreToolUse", "tool-start", true),
    ("PostToolUse", "tool-end", true),
    ("PostToolUseFailure", "tool-end", true),
    ("Notification", "notification", true),
    ("Stop", "stop", true),
    ("StopFailure", "stop-failure", true),
    ("SubagentStart", "subagent-start", true),
];

/// The one `--settings` value for Claude.
pub fn claude_settings(hook_bin: &str, permission_wait_secs: u64) -> Result<Value, LaunchError> {
    require_absolute(hook_bin)?;
    let entry = |event: &str, timeout: u64, is_async: bool| {
        let mut h = Map::new();
        h.insert("type".into(), json!("command"));
        h.insert(
            "command".into(),
            json!(hook_command(hook_bin, event, Agent::Claude)),
        );
        if is_async {
            h.insert("async".into(), json!(true));
        }
        h.insert("timeout".into(), json!(timeout));
        json!([{ "hooks": [Value::Object(h)] }])
    };
    let mut hooks = Map::new();
    for (name, arg, is_async) in CLAUDE_HOOKS {
        hooks.insert((*name).into(), entry(arg, 10, *is_async));
    }
    hooks.insert(
        "PermissionRequest".into(),
        entry(
            "permission",
            permission_timeout(permission_wait_secs),
            false,
        ),
    );
    hooks.insert("SessionEnd".into(), entry("session-end", 2, false));
    Ok(json!({ "hooks": Value::Object(hooks) }))
}

/// Top-level `--settings` keys a caller may set. They go into argv, which other users of the
/// machine can read, so they carry no secrets: `apiKeyHelper` is a command that should read
/// the key from an environment variable, and variables belong in the session environment.
/// Hooks, `disableAllHooks`, `statusLine`, the variables key and the rest stay out.
pub const ALLOWED_SETTINGS: &[&str] = &["apiKeyHelper", "model"];

/// The caller's keys with our `hooks` on top. Only `ALLOWED_SETTINGS` pass; a `hooks` key is
/// its own error.
pub fn merge_claude_settings(
    ours: Value,
    caller: &Map<String, Value>,
) -> Result<Value, LaunchError> {
    for key in caller.keys() {
        if key == "hooks" {
            return Err(LaunchError::SettingsHooks);
        }
        if !ALLOWED_SETTINGS.contains(&key.as_str()) {
            return Err(LaunchError::SettingsKey(key.clone()));
        }
    }
    let mut merged = caller.clone();
    if let Value::Object(ours) = ours {
        merged.extend(ours);
    }
    Ok(Value::Object(merged))
}

pub fn build(p: &LaunchParams) -> Result<LaunchSpec, LaunchError> {
    require_absolute(p.hook_bin)?;
    check_extra_args(p.extra_args)?;
    let id = match &p.session {
        AgentSession::New(id) | AgentSession::Resume(id) => *id,
    };
    let id_used = p.agent == Agent::Claude || matches!(p.session, AgentSession::Resume(_));
    if id_used && !valid_session_id(id) {
        return Err(LaunchError::BadSessionId(id.to_owned()));
    }
    let mut argv: Vec<String> = Vec::new();
    match p.agent {
        Agent::Claude => {
            argv.push("claude".into());
            match &p.session {
                AgentSession::New(id) => argv.extend(["--session-id".into(), (*id).into()]),
                AgentSession::Resume(id) => argv.extend(["--resume".into(), (*id).into()]),
            }
            let mut settings = claude_settings(p.hook_bin, p.permission_wait_secs)?;
            if let Some(caller) = p.claude_settings {
                settings = merge_claude_settings(settings, caller)?;
            }
            argv.extend(["--settings".into(), settings.to_string()]);
            argv.extend(p.extra_args.iter().cloned());
        }
        Agent::Codex => {
            argv.push("codex".into());
            if let AgentSession::Resume(id) = &p.session {
                argv.extend(["resume".into(), (*id).into()]);
            }
            // Codex 0.160 runs interactive turns in a shared background app server per
            // CODEX_HOME. Its hooks inherit that server's environment, not this child's, so
            // they would miss our session variables (or carry another session's). In-process
            // Codex runs the hooks with the variables below.
            argv.push(CODEX_NO_DAEMON.into());
            argv.extend(p.extra_args.iter().cloned());
        }
    }
    if let Some(prompt) = p.prompt {
        argv.push("--".into());
        argv.push(prompt.into());
    }
    let vars = vec![
        ("SUSHIAI_SOCKET".into(), p.socket.into()),
        ("SUSHIAI_SESSION_ID".into(), p.sushiai_session_id.into()),
        ("SUSHIAI_SESSION_TOKEN".into(), p.session_token.into()),
        ("SUSHIAI_AGENT".into(), p.agent.as_str().into()),
    ];
    Ok(LaunchSpec { argv, vars })
}
