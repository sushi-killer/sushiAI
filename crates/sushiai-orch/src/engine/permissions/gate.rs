use crate::engine::chat_tools;
use crate::engine::chat_tools::Kind;
use crate::engine::leases::relative_to_worktree;
use crate::engine::messages;
use crate::engine::run_control::HookContext;
use crate::engine::state::App;
use crate::policy::permissions::granted;
use crate::policy::permissions::is_protected;
use crate::policy::permissions::normalized;
use crate::policy::permissions::Subject;
use crate::policy::permissions::EDIT_TOOLS;
use crate::policy::permissions::WEB_TOOLS;
use crate::store::permissions::rules_for;
use std::path::Path;

pub(super) enum Gate {
    /// Not orchd's call: Claude Code decides as it would have.
    Pass,
    Allow,
    /// A protected path the owner allowed: the write goes to staging.
    Stage(String),
    Ask(Subject),
    /// Decided by the settings; the call is refused with this reason.
    Deny(String),
}

fn edit_target(ctx: &HookContext, input: Option<&serde_json::Value>) -> Option<String> {
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()))?;
    relative_to_worktree(&ctx.worktree, file).map(|rel| normalized(&rel))
}

fn escapes_worktree(ctx: &HookContext, input: Option<&serde_json::Value>) -> bool {
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()))
        .unwrap_or_default();
    // Only an absolute path: a relative one may start with `~` or resolve
    // against another directory.
    if !Path::new(file).is_absolute() {
        return true;
    }
    let Some(rel) = relative_to_worktree(&ctx.worktree, file) else {
        return true;
    };
    // No `..` and no symlink on the way: a link may point out of the
    // worktree, also to a file that does not exist yet.
    let mut at = ctx.worktree.clone();
    for part in rel.split('/') {
        if part == ".." {
            return true;
        }
        at.push(part);
        if at
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return true;
        }
    }
    false
}

/// `mcp__server__tool` of a connected tool, read or write; other servers are
/// judged by the tool's name alone.
fn mcp_kind(app: &App, tool_name: &str) -> Kind {
    let Some((server, tool)) = tool_name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    else {
        return Kind::Write;
    };
    let settings = app.settings.read().unwrap().clone();
    let cfg = settings
        .chat_tools
        .iter()
        .find(|c| chat_tools::sanitize(&chat_tools::server_key(c)) == server);
    let overrides = cfg.map(|c| c.overrides.clone()).unwrap_or_default();
    let known = cfg
        .and_then(|c| chat_tools::load_cache(app).remove(&c.id))
        .and_then(|d| {
            d.tools
                .into_iter()
                .find(|t| chat_tools::sanitize(&t.name) == tool)
        });
    match known {
        Some(t) => chat_tools::classify(&t.name, t.read_only, &overrides),
        None => chat_tools::classify(tool, None, &overrides),
    }
}

pub(super) fn gate_for(
    app: &App,
    ctx: &HookContext,
    tool: &str,
    input: Option<&serde_json::Value>,
) -> Gate {
    let rules = rules_for(&app.data_dir, &ctx.repo, &ctx.task_id);
    if EDIT_TOOLS.contains(&tool) {
        let Some(rel) = edit_target(ctx, input) else {
            return Gate::Pass;
        };
        if !is_protected(&rel) {
            // Headless Claude Code on Linux refused edits of files in the
            // task's own worktree; an explicit allow keeps that from parking
            // the task on an owner question. A path that climbs out is not
            // the worktree's and stays with Claude's own rules.
            return if escapes_worktree(ctx, input) {
                Gate::Pass
            } else {
                Gate::Allow
            };
        }
        let subject = Subject::Path(rel.clone());
        let ran = ctx.run_grants.lock().unwrap().contains(&subject.rule());
        return if ran || granted(&rules, &subject.rule()) {
            Gate::Stage(rel)
        } else {
            Gate::Ask(subject)
        };
    }
    if tool.starts_with(&format!("mcp__{}__", messages::SERVER)) {
        return Gate::Pass;
    }
    let subject = Subject::Tool(tool.to_string());
    if tool.starts_with("mcp__") {
        if mcp_kind(app, tool) == Kind::Read
            || granted(&rules, &subject.rule())
            || ctx.run_grants.lock().unwrap().contains(&subject.rule())
        {
            return Gate::Allow;
        }
        return Gate::Ask(subject);
    }
    if WEB_TOOLS.contains(&tool) {
        if granted(&rules, &subject.rule())
            || ctx.run_grants.lock().unwrap().contains(&subject.rule())
            || web_allowed(&app.settings.read().unwrap().allowed_domains, tool, input)
        {
            return Gate::Allow;
        }
        return Gate::Deny(format!(
            "{tool} is limited to the allowed domains in the settings and this call is outside them. Meet the task without it, or report `blocked` and say what you could not read."
        ));
    }
    Gate::Pass
}

/// Whether the allowed domains cover a web call: `*` covers everything, a
/// `WebFetch` is covered by its host, and a `WebSearch` has no host.
pub(super) fn web_allowed(
    domains: &[String],
    tool: &str,
    input: Option<&serde_json::Value>,
) -> bool {
    if domains.iter().any(|d| d.trim() == "*") {
        return true;
    }
    let host = (tool == "WebFetch")
        .then(|| input.and_then(|i| i.get("url")).and_then(|u| u.as_str()))
        .flatten()
        .and_then(|url| url::Url::parse(url).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    let Some(host) = host else {
        return false;
    };
    domains.iter().any(|d| {
        let d = d.trim().trim_start_matches("*.").to_ascii_lowercase();
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    })
}
