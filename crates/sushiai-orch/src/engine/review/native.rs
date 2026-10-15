use crate::engine::state::App;
use crate::harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::store;
use std::path::Path;
use std::path::PathBuf;

/// The run request of a read-only session: review's, and anything that must
/// be held to the same restrictions (no edits, no network, never resumed,
/// no MCP servers but the empty config it is handed).
pub(in crate::engine) fn review_request<'a>(
    route: &'a Route,
    worktree: &'a Path,
    mcp_config: &'a Path,
    settings_path: &'a Path,
    images: &'a [PathBuf],
) -> harness::RunRequest<'a> {
    harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        max_budget_usd: None,
        review: true,
        task_run: false,
        task_sandbox: None,
        mcp_config: Some(mcp_config),
        settings_path: Some(settings_path),
        network_allowed: false,
        codex_mcp: &[],
        images,
        repo_settings: true,
        resume: None,
    }
}

/// The start of a reply, at most `max` characters, with an ellipsis when cut.
pub(in crate::engine) fn head_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

/// Writes `settings.json` for a read-only Claude session (no Stop hook --
/// used by the plan stage and orchestrator triage, never the implement
/// path, which additionally wires one up) that still carries its route's
/// profile (env/API key) the same way an implement attempt would. A no-op
/// for a Codex route, which has no such settings file.
pub(in crate::engine) fn write_readonly_claude_settings_with_profile(
    route: &Route,
    app: &App,
    settings: &Settings,
    deny_read: &[String],
    settings_path: &Path,
) {
    let key_path = settings_path.with_file_name("key");
    let key_path = key_path.as_path();
    let profile = route
        .profile_id
        .as_ref()
        .and_then(|pid| app.secrets.read().unwrap().profiles.get(pid).cloned());
    let mut profile_obj = serde_json::Map::new();
    if let Some(p) = &profile {
        if !p.env.is_empty() {
            profile_obj.insert(
                "env".to_string(),
                serde_json::to_value(&p.env).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(key) = &p.key {
            if store::files::write_secret_file(key_path, key).is_ok() {
                profile_obj.insert(
                    "apiKeyHelper".to_string(),
                    serde_json::Value::String(format!(
                        "cat {}",
                        harness::shell_quote(&key_path.to_string_lossy())
                    )),
                );
            }
        }
    }
    if let Some(account) = route
        .account_id
        .as_ref()
        .and_then(|id| app.secrets.read().unwrap().accounts.get(id).cloned())
    {
        if account.kind == "apiKey" {
            if store::files::write_secret_file(key_path, &account.value).is_ok() {
                profile_obj.insert(
                    "apiKeyHelper".into(),
                    serde_json::Value::String(format!(
                        "cat {}",
                        harness::shell_quote(&key_path.to_string_lossy())
                    )),
                );
            }
        } else if account.kind == "subscription" {
            let _ = store::files::write_secret_file(
                &key_path.with_file_name("oauth-token"),
                &account.value,
            );
        }
    }
    let profile_value = if profile_obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(profile_obj))
    };
    let claude_settings = harness::build_claude_settings(
        profile_value.as_ref(),
        settings.sandbox,
        &settings.allowed_domains,
        deny_read,
        None,
    );
    let _ = store::files::write_json_atomic(settings_path, &claude_settings);
}
