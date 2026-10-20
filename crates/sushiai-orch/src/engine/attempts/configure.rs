use crate::engine::attempts::cache::apply_agent_cache;
use crate::engine::state::App;
use crate::harness;
use crate::harness::compiler_cache;
use crate::harness::environment::augmented_path;
use crate::policy::settings::SandboxMode;
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

pub(super) fn configure_command(
    app: &App,
    task_id: &str,
    req: &harness::RunRequest<'_>,
    project_env: &HashMap<String, String>,
    cmd: &mut tokio::process::Command,
) {
    let worktree = cmd
        .as_std()
        .get_current_dir()
        .expect("run current directory")
        .to_path_buf();
    let path = augmented_path();
    cmd.env("PATH", &path)
        // Lets a repo's own hooks tell an orchd-run agent from a person's
        // session (sushiAI's lesson reminder stays quiet for it).
        .env("ORCHD_TASK", task_id)
        .env("SUSHIAI_HOME", &app.home)
        .envs(project_env);
    // Before the first attempt builds anything: one target directory per
    // worktree, recorded once in the task's decisions.
    if let Some(line) = crate::cargo_target::isolate(cmd.as_std_mut(), &worktree) {
        record_once(app, task_id, line);
    }
    let task_policy = req
        .task_sandbox
        .filter(|_| cfg!(target_os = "macos") && req.task_run && !req.review);
    if let Some(policy) = task_policy {
        // The task sandbox policy owns the variables of a sandboxed run.
        policy.apply_check_env(cmd.as_std_mut());
    } else if !project_env.contains_key("RUSTC_WRAPPER") {
        let cache = compiler_cache::CompilerCache::for_worktree(&worktree, &path);
        let sandbox = if req.task_run && !req.review {
            SandboxMode::Host
        } else {
            app.settings.read().unwrap().sandbox
        };
        let owner_wrapper = std::env::var_os("RUSTC_WRAPPER").is_some();
        if let Some(line) = apply_agent_cache(cmd, cache.as_ref(), req, sandbox, owner_wrapper) {
            record_once(app, task_id, line);
        }
    }
}

/// Appends a decision line unless the task already has it.
fn record_once(app: &App, task_id: &str, line: String) {
    // Best-of candidates can discover the same decision concurrently.
    static DECISIONS: StdMutex<()> = StdMutex::new(());
    let _write = DECISIONS.lock().unwrap();
    let recorded = app
        .store
        .load_task(task_id)
        .ok()
        .flatten()
        .is_some_and(|task| task.decisions.contains(&line));
    if !recorded {
        app.append_decision(task_id, line);
    }
}
