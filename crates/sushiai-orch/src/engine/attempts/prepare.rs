use crate::engine::attempts::credentials::claude_profile;
use crate::engine::messages;
use crate::engine::run_control::HookContext;
use crate::engine::state::App;
use crate::harness;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::sandbox::TaskSandbox;
use crate::store;
use crate::task::state::Task;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// The files and hook registration one implement run needs: its MCP config,
/// its Claude `settings.json` (with the Stop-hook gate registered for
/// `worktree`), and the per-run key file.
pub(in crate::engine) struct PreparedRun {
    pub mcp_path: PathBuf,
    pub settings_path: PathBuf,
    pub key_path: PathBuf,
    pub codex_mcp: Vec<(String, serde_json::Value)>,
    pub registered: Option<(String, Arc<HookContext>)>,
    pub sandbox: Option<TaskSandbox>,
}

/// The selected implement run whose native configuration is being prepared.
#[derive(Clone, Copy)]
pub(in crate::engine) struct ImplementRun<'a> {
    pub(in crate::engine) task: &'a Task,
    pub(in crate::engine) route: &'a Route,
    pub(in crate::engine) settings: &'a Settings,
    pub(in crate::engine) worktree: &'a Path,
    pub(in crate::engine) attempt_n: u32,
    pub(in crate::engine) run_dir: &'a Path,
}

impl ImplementRun<'_> {
    pub(in crate::engine) fn request<'a>(
        &'a self,
        prepared: &'a PreparedRun,
        resume: Option<&'a str>,
    ) -> harness::RunRequest<'a> {
        let cap = self.task.variant().max_attempt_cost_usd;
        harness::RunRequest {
            harness: self.route.harness,
            worktree: self.worktree,
            model: self.route.model.as_deref(),
            effort: self.route.effort.as_deref(),
            max_budget_usd: (cap > 0.0).then_some(cap),
            review: false,
            task_run: true,
            task_sandbox: prepared.sandbox.as_ref(),
            mcp_config: Some(&prepared.mcp_path),
            settings_path: Some(&prepared.settings_path),
            network_allowed: self.settings.codex_network,
            codex_mcp: &prepared.codex_mcp,
            images: &[],
            repo_settings: true,
            resume,
        }
    }

    pub(in crate::engine) fn guard(&self, prepared: &PreparedRun) -> crate::harness::guard::Guard {
        let started_at = self
            .task
            .attempts
            .iter()
            .rev()
            .find(|attempt| attempt.n == self.attempt_n)
            .map_or_else(crate::clock::now_ms, |attempt| attempt.started_at);
        crate::harness::guard::Guard::for_variant(
            &self.task.variant(),
            prepared
                .registered
                .as_ref()
                .map(|(_, ctx)| ctx.hook_running.clone()),
        )
        .with_time_budget(self.settings.implement_budget_mins, started_at)
    }
}

pub(in crate::engine) fn prepare_run(
    app: &Arc<App>,
    run: &ImplementRun<'_>,
    tools: &crate::engine::permissions::tools::TaskTools,
) -> PreparedRun {
    let ImplementRun {
        task,
        route,
        settings,
        worktree,
        attempt_n,
        run_dir,
    } = *run;
    let task_id = task.id.as_str();
    let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
    let mcp_path = run_dir.join("mcp.json");
    // The repo's project servers, the connected tools and the task's own,
    // plus orchd's messaging bridge scoped to this task.
    let messages_server = messages::task_server(app, task_id);
    let _ = store::files::write_json_private(&mcp_path, &tools.mcp_config(&messages_server));

    let token = uuid::Uuid::new_v4().to_string();
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    // Keep our own clone of the hook context alongside the one handed
    // to `hook.stop` lookups, so we can read back how many times it
    // blocked into `attempt.gateBlocks` once the run ends.
    let mut registered: Option<(String, Arc<HookContext>)> = None;
    if matches!(route.harness, Harness::Claude) {
        let profile_value = claude_profile(app, route, run_dir, &key_path);

        let home_str = app.home.to_string_lossy().to_string();
        let stop_hook = harness::StopHook {
            exe_path: &app.exe_path,
            home: &home_str,
            token: &token,
        };
        let mut claude_settings = harness::build_claude_task_settings(
            profile_value.as_ref(),
            sandbox.as_ref(),
            &settings.allowed_domains,
            Some(stop_hook),
        );
        // A headless run answers no prompts, so the messaging tools and the
        // read tools of the connected tools must be allowed up front to be
        // usable at all. A write tool never is: the hook asks the owner.
        if let Some(allow) = claude_settings["permissions"]["allow"].as_array_mut() {
            for tool in crate::api::mcp::TASK_TOOLS {
                allow.push(json!(format!("mcp__{}__{tool}", messages::SERVER)));
            }
            allow.extend(tools.allowed().into_iter().map(serde_json::Value::String));
        }
        let _ = store::files::write_json_private(&settings_path, &claude_settings);
        let ctx = HookContext::new(task, attempt_n, worktree, &task.base_sha);
        app.register_hook(&token, &ctx, run_dir);
        registered = Some((token.clone(), ctx));
    }
    let codex_mcp = tools.codex_servers((sandbox.is_none()).then_some(&messages_server));
    PreparedRun {
        mcp_path,
        settings_path,
        key_path,
        codex_mcp,
        registered,
        sandbox,
    }
}
