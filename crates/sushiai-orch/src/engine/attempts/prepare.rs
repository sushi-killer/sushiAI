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
use crate::task::Task;
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
    pub messages_server: serde_json::Value,
    pub registered: Option<(String, Arc<HookContext>)>,
    pub sandbox: Option<TaskSandbox>,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) fn prepare_run(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    route: &Route,
    settings: &Settings,
    worktree: &Path,
    attempt_n: u32,
    run_dir: &Path,
    tools: &crate::engine::permissions::tools::TaskTools,
) -> PreparedRun {
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
    PreparedRun {
        mcp_path,
        settings_path,
        key_path,
        messages_server,
        registered,
        sandbox,
    }
}
