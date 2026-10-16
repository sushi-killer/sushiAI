use super::{claude::claude_argv, codex::codex_argv};
use crate::policy::routing::Harness;
use crate::sandbox::TaskSandbox;
use std::path::Path;

pub struct RunRequest<'a> {
    pub harness: Harness,
    pub worktree: &'a Path,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// Claude only: `--max-budget-usd`. Every harness: orchd itself stops the
    /// run once its streamed usage, priced with `settings.prices`, exceeds it.
    pub max_budget_usd: Option<f64>,
    /// Read-only review session instead of an implement session.
    pub review: bool,
    /// Only task implement runs use the orchd policy (including host overrides).
    pub task_run: bool,
    pub task_sandbox: Option<&'a TaskSandbox>,
    /// Claude only: paths to the run's `mcp.json` / `settings.json`.
    pub mcp_config: Option<&'a Path>,
    pub settings_path: Option<&'a Path>,
    /// Codex only: whether the sandbox has network access
    /// (`sandbox_workspace_write.network_access`). Task implement shell
    /// commands use the outer profile and have open network regardless.
    pub network_allowed: bool,
    /// Codex only: MCP servers (name, `{command, args, env?}` or `{url}`)
    /// passed as `-c mcp_servers.*` flags; Claude reads its servers from
    /// `mcp_config`.
    pub codex_mcp: &'a [(String, serde_json::Value)],
    /// Codex only: images attached to the prompt (`--image`); Claude opens
    /// image files itself with its Read tool.
    pub images: &'a [std::path::PathBuf],
    /// Claude only: load the checkout's own `.claude/settings{,.local}.json`
    /// (`--setting-sources project,local`). Off for a run in a checkout
    /// orchd does not own (an audited repo): those settings carry hooks,
    /// shell commands that run outside the sandbox.
    pub repo_settings: bool,
    /// Continue this earlier session instead of starting one: Claude gets
    /// `--resume <id>`, Codex `exec resume <id>`.
    pub resume: Option<&'a str>,
}

/// The JSON schema an agent's final reply must match: Claude gets the text
/// (`--json-schema`), Codex the file it reads (`--output-schema`).
#[derive(Clone, Copy)]
pub struct OutputSchema<'a> {
    pub json: &'a str,
    pub path: &'a Path,
}

pub fn build_argv(req: &RunRequest) -> Vec<String> {
    build_argv_with_schema(req, None)
}

/// [`build_argv`] plus the schema flag for the harness. Codex reads its prompt
/// from stdin after the flags, so the trailing `-` stays last.
pub fn build_argv_with_schema(req: &RunRequest, schema: Option<&OutputSchema>) -> Vec<String> {
    let mut argv = match req.harness {
        Harness::Claude => claude_argv(req),
        Harness::Codex => codex_argv(req),
    };
    if let Some(schema) = schema {
        match req.harness {
            Harness::Claude => {
                argv.push("--json-schema".to_string());
                argv.push(schema.json.to_string());
            }
            Harness::Codex => {
                let stdin = argv.pop();
                argv.push("--output-schema".to_string());
                argv.push(schema.path.to_string_lossy().to_string());
                argv.extend(stdin);
            }
        }
    }
    argv
}
