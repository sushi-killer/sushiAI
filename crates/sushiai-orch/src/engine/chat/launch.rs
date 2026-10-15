use super::store::turn_dir;
use super::turn::{ScheduledTurn, Turn};
use super::types::ChatMode;
use crate::engine::chat_tools;
use crate::engine::review::write_readonly_claude_settings_with_profile;
use crate::engine::state::App;
use crate::harness;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::store;
use serde_json::json;
use std::path::Path;

pub(super) const SERVER: &str = "sushiai-orchestrator";

/// The orchestrator agent's route: the one the owner picked, else the route
/// that handles standard tasks.
pub(in crate::engine) fn orchestrator_route(settings: &Settings) -> Option<Route> {
    let find = |id: &str| settings.routes.iter().find(|r| r.id == id).cloned();
    find(&settings.orchestrator).or_else(|| find(settings.tiers.get(&Tier::Standard)?))
}

pub(super) fn mcp_server(
    app: &App,
    task_mcp_file: Option<&Path>,
    read_only: bool,
    overseer: bool,
) -> serde_json::Value {
    let mut server = json!({
        "command": app.exe_path,
        "args": ["mcp"],
        "env": {"SUSHIAI_HOME": app.home.to_string_lossy()},
    });
    server["env"]["ORCHD_BRIDGE_SOURCE"] = json!(if overseer { "overseer" } else { "chat" });
    if read_only {
        server["args"]
            .as_array_mut()
            .expect("args is an array")
            .push(json!("--read-only"));
    }
    if let Some(file) = task_mcp_file {
        server["env"]["ORCHD_TASK_MCP"] = json!(file.to_string_lossy());
    }
    server
}

pub(super) struct ChatInvocation<'a> {
    pub(super) route: &'a Route,
    pub(super) session: Option<&'a str>,
    pub(super) mode: ChatMode,
    pub(super) read_only: bool,
}

pub(super) fn claude_argv(
    invocation: &ChatInvocation<'_>,
    mcp_config: &Path,
    settings_path: &Path,
    connected: &chat_tools::TurnTools,
) -> Vec<String> {
    let ChatInvocation {
        route,
        session,
        mode,
        read_only,
    } = *invocation;
    let offered: &[&str] = if read_only {
        &crate::api::mcp::READ_ONLY_TOOLS
    } else {
        &crate::api::mcp::ORCHESTRATOR_TOOLS
    };
    let tools = offered
        .iter()
        .map(|t| format!("mcp__{SERVER}__{t}"))
        .chain(connected.allowed.iter().cloned())
        .collect::<Vec<_>>()
        .join(",");
    let prompt = format!("{}{}", mode.prompt(), connected.prompt);
    let mut argv: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--setting-sources",
        "project,local",
        "--disable-slash-commands",
        "--tools",
        "Read,Grep,Glob",
        "--append-system-prompt",
        &prompt,
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.push(mcp_config.to_string_lossy().to_string());
    argv.push("--settings".to_string());
    argv.push(settings_path.to_string_lossy().to_string());
    argv.push("--allowedTools".to_string());
    argv.push(tools);
    argv.push("--permission-prompts".to_string());
    argv.push("none".to_string());
    if let Some(model) = &route.model {
        argv.push("--model".to_string());
        argv.push(model.clone());
    }
    if let Some(effort) = &route.effort {
        argv.push("--effort".to_string());
        argv.push(effort.clone());
    }
    if let Some(session) = session {
        argv.push("--resume".to_string());
        argv.push(session.to_string());
    }
    argv
}

pub(super) fn codex_argv(
    invocation: &ChatInvocation<'_>,
    repo: &str,
    server: &serde_json::Value,
    connected: &chat_tools::TurnTools,
) -> Vec<String> {
    let ChatInvocation {
        route,
        session,
        mode,
        ..
    } = *invocation;
    let mut argv: Vec<String> = match session {
        // `resume` takes neither `-C` nor `--sandbox`; the cwd is set on the
        // process and the sandbox through config.
        Some(session) => vec![
            "exec".into(),
            "resume".into(),
            session.into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-c".into(),
            "sandbox_mode=\"read-only\"".into(),
        ],
        None => vec![
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-C".into(),
            repo.into(),
            "--sandbox".into(),
            "read-only".into(),
        ],
    };
    let toml = |v: &serde_json::Value| v.to_string();
    argv.push("-c".into());
    argv.push(format!(
        "developer_instructions={}",
        toml(&json!(format!("{}{}", mode.prompt(), connected.prompt)))
    ));
    argv.extend(harness::codex_mcp_flags(SERVER, server));
    argv.extend(chat_tools::codex_flags(connected));
    if let Some(model) = &route.model {
        argv.push("-m".into());
        argv.push(model.clone());
    }
    if let Some(effort) = &route.effort {
        argv.push("-c".into());
        argv.push(format!("model_reasoning_effort=\"{effort}\""));
    }
    argv.push("-".into());
    argv
}

#[cfg(test)]
#[path = "tests/launch.rs"]
mod tests;

pub(super) struct PreparedLaunch {
    pub(super) argv: Vec<String>,
    pub(super) key_path: std::path::PathBuf,
    pub(super) settings: Settings,
    pub(super) mode: ChatMode,
    pub(super) resumed: bool,
}

pub(super) async fn prepare(app: &App, scheduled: &ScheduledTurn) -> PreparedLaunch {
    let ScheduledTurn {
        repo,
        thread,
        route,
        turn,
        ..
    } = scheduled;
    let mode = match turn {
        Turn::Owner(mode) => *mode,
        Turn::Questions(_) | Turn::Overseer { .. } => ChatMode::Chat,
    };
    let read_only = mode.read_only()
        || matches!(
            turn,
            Turn::Overseer {
                read_only: true,
                ..
            }
        );
    let dir = turn_dir(app, repo);
    let _ = std::fs::create_dir_all(&dir);
    // Brainstorm and Plan never create tasks, so they have no task MCP to hand on.
    let task_mcp = thread.task_mcp.as_ref().filter(|_| !read_only);
    let task_mcp_file = task_mcp.and_then(|mcp| {
        let file = dir.join("task-mcp.json");
        store::files::write_json_atomic(&file, &json!({"repo": repo, "mcp": mcp}))
            .ok()
            .map(|_| file)
    });
    let server = mcp_server(
        app,
        task_mcp_file.as_deref(),
        read_only,
        matches!(turn, Turn::Overseer { .. }),
    );
    let settings = app.settings.read().unwrap().clone();
    // A question turn answers a task, not the owner: it gets no connected tools.
    let connected = match turn {
        Turn::Owner(_) => chat_tools::for_turn(app, &settings).await,
        Turn::Questions(_) | Turn::Overseer { .. } => chat_tools::TurnTools::default(),
    };
    // Only the owner's turns resume the harness session; a wake starts fresh.
    let owner = matches!(turn, Turn::Owner(_));
    let session = thread.session_id.clone().filter(|_| owner);
    let key_path = dir.join("key");
    let argv = match route.harness {
        Harness::Claude => {
            let mcp_config = dir.join("mcp.json");
            let mut servers = serde_json::Map::new();
            for (key, def) in &connected.servers {
                servers.insert(key.clone(), def.clone());
            }
            servers.insert(SERVER.to_string(), server);
            let _ = store::files::write_json_atomic(&mcp_config, &json!({"mcpServers": servers}));
            let settings_path = dir.join("settings.json");
            write_readonly_claude_settings_with_profile(
                route,
                app,
                &key_path,
                &settings,
                &[app.data_dir.to_string_lossy().to_string()],
                &settings_path,
            );
            claude_argv(
                &ChatInvocation {
                    route,
                    session: session.as_deref(),
                    mode,
                    read_only,
                },
                &mcp_config,
                &settings_path,
                &connected,
            )
        }
        Harness::Codex => codex_argv(
            &ChatInvocation {
                route,
                session: session.as_deref(),
                mode,
                read_only: true,
            },
            repo,
            &server,
            &connected,
        ),
    };

    PreparedLaunch {
        argv,
        key_path,
        settings,
        mode,
        resumed: session.is_some(),
    }
}
