//! Claude's `--settings` JSON: the status hooks and the permission approver,
//! plus the few top-level keys a caller may add. One `--settings` value only,
//! because a second `--settings` flag is undocumented.

use serde_json::{json, Map, Value};

use crate::harness::launch::{permission_timeout, require_absolute, shell_quote, LaunchError};

/// The hook command line: `<bin> hook <event>`. Claude needs no `--agent` here;
/// the daemon tells it which agent it is through the session variables.
pub fn hook_command(bin: &str, event: &str) -> String {
    format!("{} hook {}", shell_quote(bin), event)
}

/// (Claude event, hook argument, async). Status-only hooks run async so
/// they never slow the agent; the ones that answer or must finish first do not.
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
        h.insert("command".into(), json!(hook_command(hook_bin, event)));
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
