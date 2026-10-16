use crate::engine::secrets;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use std::collections::HashMap;
use std::time::Duration;

/// The project's agent variables, and every value known for the project
/// (agent and MCP variables) that a run's output must not keep.
#[derive(Default)]
pub(crate) struct RunSecrets {
    pub env: HashMap<String, String>,
    pub redaction: HashMap<String, String>,
}

/// How long a run of a task with a project waits for the app's values after
/// a daemon start before it goes on without them.
const APP_SECRETS_WAIT: Duration = Duration::from_secs(120);

/// After a daemon restart the app sends project values only once it has
/// reconnected; a task with a project that resumes first waits for them
/// instead of failing its project check on empty maps.
pub(crate) async fn wait_for_app_secrets(app: &App, task_id: &str, cancel: &CancelToken) {
    let has_project = app
        .store
        .load_task(task_id)
        .ok()
        .flatten()
        .is_some_and(|task| task.project_id.is_some());
    let deadline = tokio::time::Instant::now() + APP_SECRETS_WAIT;
    while has_project
        && !app.secrets.read().unwrap().received
        && tokio::time::Instant::now() < deadline
        && !cancel.is_cancelled()
    {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub(crate) fn run_secrets(app: &App, task_id: &str) -> Result<RunSecrets, String> {
    let Some(task) = app.store.load_task(task_id).ok().flatten() else {
        return Ok(Default::default());
    };
    let secrets = app.secrets.read().unwrap();
    let id = secrets::project_for_repo(&secrets, &task.repo, task.project_id.as_deref())?;
    Ok(id.map_or_else(Default::default, |id| {
        let agent = secrets.projects.get(&id).cloned().unwrap_or_default();
        let mut known = agent.clone();
        known.extend(secrets.project_mcp.get(&id).cloned().unwrap_or_default());
        RunSecrets {
            env: agent,
            redaction: known,
        }
    }))
}
