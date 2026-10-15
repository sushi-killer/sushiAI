use crate::engine::chat_tools::TurnTools;
use crate::engine::messages;
use crate::engine::permissions::brief::tools_block;
use crate::engine::permissions::tools::task_tools;
use crate::engine::permissions::tools::TaskTools;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::policy::settings::Settings;
use crate::store::permissions::add_rule;
use crate::store::permissions::Scope;
use crate::task::TaskStatus;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn task_tools_drop_full_bridges_from_every_source() {
    let exe = "/opt/bin/renamed-daemon";
    let full = json!({"command": exe, "args": ["mcp"], "codex": true});
    let readonly =
        json!({"command": "/opt/bin/sushiai", "args": ["mcp", "--read-only"], "codex": true});
    let scoped =
        json!({"command": "sushiai", "args": ["mcp", "--task", "other-task"], "codex": true});
    let unrelated = json!({"command": "sushiai", "args": ["orch", "status"], "codex": true});
    let tools = TaskTools {
        connected: TurnTools {
            servers: [("connected-full".into(), full.clone())].into(),
            listed: vec![("connected-full".into(), vec!["get".into()], vec![])],
            ..Default::default()
        },
        project: [
            ("project-full".into(), full.clone()),
            ("project-readonly".into(), readonly.clone()),
        ]
        .into(),
        task: [
            ("task-full".into(), full),
            ("task-readonly".into(), readonly),
            ("scoped".into(), scoped),
            ("unrelated".into(), unrelated),
            (
                messages::SERVER.into(),
                json!({"command": exe, "args": ["mcp"]}),
            ),
        ]
        .into(),
        off: vec![],
        granted: vec![],
        env: HashMap::new(),
        notes: vec![],
        exe_path: exe.into(),
    };
    let messages = json!({"command": exe, "args": ["mcp", "--task", "this-task"]});
    let config = tools.mcp_config(&messages);
    let servers = config["mcpServers"].as_object().unwrap();
    let codex = tools.codex_servers(Some(&messages));
    let lines = tools.lines().join("\n");
    for name in [
        "connected-full",
        "project-full",
        "project-readonly",
        "task-full",
        "task-readonly",
    ] {
        assert!(!servers.contains_key(name), "{config}");
        assert!(!codex.iter().any(|(key, _)| key == name));
        assert!(!lines.contains(name), "{lines}");
    }
    for name in ["scoped", "unrelated"] {
        assert!(servers.contains_key(name), "{config}");
        assert!(codex.iter().any(|(key, _)| key == name));
        assert!(lines.contains(name), "{lines}");
    }
    assert_eq!(servers[messages::SERVER], messages);
    assert_eq!(codex.last().unwrap(), &(messages::SERVER.into(), messages));
}

#[test]
fn a_grant_reaches_the_run_but_a_write_tool_is_never_pre_allowed() {
    let tools = TaskTools {
        connected: TurnTools {
            allowed: vec!["mcp__a__get".into()],
            servers: [("a".to_string(), json!({"command": "x"}))].into(),
            listed: vec![("a".into(), vec!["get".into()], vec!["send".into()])],
            ..Default::default()
        },
        off: vec![("asana".into(), "Asana".into())],
        project: [
            ("a".to_string(), json!({"command": "project"})),
            ("p".to_string(), json!({"command": "p"})),
        ]
        .into(),
        task: [
            ("t".to_string(), json!({"command": "t", "codex": true})),
            // The app's snapshot of a configured server: Claude-only.
            ("p".to_string(), json!({"command": "p"})),
            (
                "sushiai-orchestrator".to_string(),
                json!({"command": "o", "codex": true}),
            ),
        ]
        .into(),
        granted: vec!["mcp__a__send".into(), "mcp__a__get".into()],
        env: HashMap::new(),
        exe_path: "sushiai".into(),
        notes: vec![],
    };
    assert_eq!(tools.allowed(), vec!["mcp__a__get", "mcp__a__send"]);
    let config = tools.mcp_config(&json!({"command": "messages"}));
    let servers = config["mcpServers"].as_object().unwrap();
    // The repo's own definition of a server wins over the connected one.
    assert_eq!(servers["a"]["command"], "project");
    assert!(servers.contains_key("p") && servers.contains_key("t"));
    assert!(servers["t"].get("codex").is_none());
    assert_eq!(servers[messages::SERVER]["command"], "messages");
    // Codex gets only the task's own servers and the messages server:
    // it has no hook to ask the owner before a connected tool writes.
    let codex: Vec<String> = tools
        .codex_servers(Some(&json!({"command": "messages"})))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(codex, vec!["t".to_string(), messages::SERVER.to_string()]);
    let sandboxed: Vec<String> = tools
        .codex_servers(None)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(sandboxed, vec!["t".to_string()]);
    let t = &tools.codex_servers(Some(&json!({})))[0].1;
    assert!(t.get("codex").is_none());
    let block = tools_block(&tools, false);
    assert!(
        block.contains("a: read tools get; write tools send"),
        "{block}"
    );
    assert!(block.contains("No tool: "), "{block}");
    assert!(!tools_block(&tools, true).contains("No tool: "));
}

#[tokio::test]
async fn task_tools_project_id_uses_only_snapshot_and_no_id_falls_back_to_worktree() {
    let (app, dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".mcp.json"),
        json!({"mcpServers": {
            "fallback": {"command": "fallback"}
        }})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        app.store.task_dir(&task.id).join("mcp.json"),
        json!({"mcpServers": {
            "snapshot": {"command": "snapshot", "codex": true},
            "remove": {"command": "remove", "codex": true},
            "sushiai-messages": {"command": "fake", "codex": true}
        }})
        .to_string(),
    )
    .unwrap();
    let mut settings = Settings::default();
    let tools = task_tools(&app, &settings, &task, &worktree);
    assert!(tools.project.contains_key("fallback"));
    settings.mcp_disable.insert(
        task.repo.clone(),
        vec!["remove".into(), messages::SERVER.into(), "missing".into()],
    );
    settings
        .mcp_enable
        .insert(task.repo.clone(), vec!["not-a-tool".into()]);
    let tools = task_tools(&app, &settings, &task, &worktree);
    let config = tools.mcp_config(&json!({"command": "messages"}));
    assert_eq!(
        config["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["fallback", "snapshot", messages::SERVER]
    );
    let codex = tools.codex_servers(Some(&json!({"command": "messages"})));
    assert_eq!(
        codex
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["snapshot", messages::SERVER]
    );
    assert_eq!(codex.last().unwrap().1["command"], "messages");
    assert_eq!(
        config["mcpServers"][messages::SERVER]["command"],
        "messages"
    );
    assert_eq!(
        tools.notes,
        vec![
            format!(
                "MCP: ignored unknown name \"not-a-tool\" in mcpEnable for {}",
                task.repo
            ),
            format!(
                "MCP: ignored unknown name \"missing\" in mcpDisable for {}",
                task.repo
            ),
        ]
    );
    app.secrets
        .write()
        .unwrap()
        .repo_projects
        .insert(task.repo.clone(), "project".into());
    assert!(task_tools(&app, &settings, &task, &worktree)
        .project
        .is_empty());
    task.fixture_data().project_id = Some("project".into());
    assert!(task_tools(&app, &settings, &task, &worktree)
        .project
        .is_empty());
    // A stored project id that does not match the repo is no project.
    app.secrets.write().unwrap().repo_projects.clear();
    assert!(task_tools(&app, &settings, &task, &worktree)
        .project
        .contains_key("fallback"));
    settings
        .mcp_disable
        .insert(task.repo.clone(), vec!["fallback".into()]);
    task.fixture_data().project_id = None;
    assert!(task_tools(&app, &settings, &task, &worktree)
        .project
        .is_empty());
}

#[tokio::test]
async fn task_tools_enable_and_grants_are_repo_scoped_and_disable_matches_id_or_server_key() {
    let (app, dir) = test_app();
    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let mut settings = Settings {
        chat_tools: serde_json::from_value(json!([
            {"id": "global", "label": "Global tool", "enabled": true, "server": {"command": "true"}},
            {"id": "local", "label": "Local tool", "enabled": false, "server": {"command": "true"}},
            {"id": "plug", "label": "Plugin tool", "enabled": true, "server": {"ref": "plugin:demo/server"}}
        ])).unwrap(),
        ..Settings::default()
    };
    std::fs::write(app.data_dir.join("chat-tools.json"), json!({
        "global": {"status": "ok", "tools": [{"name": "get_item", "readOnly": true}], "checkedAt": 0},
        "local": {"status": "ok", "tools": [{"name": "get_item", "readOnly": true}], "checkedAt": 0}
    }).to_string()).unwrap();
    settings
        .mcp_enable
        .insert(format!("{}/", task.repo), vec!["global".into()]);
    let tools = task_tools(&app, &settings, &task, dir.path());
    assert!(tools.connected.servers.is_empty());
    assert!(tools_block(&tools, false)
        .contains("global (Global tool): connected but off for this repo"));
    assert_eq!(
        tools.capabilities()["connectedButOffForThisTask"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    settings
        .mcp_enable
        .insert(task.repo.clone(), vec!["local".into()]);
    add_rule(&app.data_dir, Scope::Repo(&task.repo), "enable:global");
    let tools = task_tools(&app, &settings, &task, dir.path());
    assert_eq!(
        tools
            .connected
            .servers
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["global", "local"]
    );
    assert_eq!(tools.off, vec![("plug".into(), "Plugin tool".into())]);
    add_rule(&app.data_dir, Scope::Task(&task.id), "enable:local");
    settings.mcp_enable.clear();
    assert!(task_tools(&app, &settings, &task, dir.path())
        .connected
        .servers
        .contains_key("local"));

    std::fs::write(
        app.store.task_dir(&task.id).join("mcp.json"),
        json!({"mcpServers": {
            "global": {"command": "task-copy", "codex": true},
            "plugin:demo:server": {"command": "plugin-copy", "codex": true}
        }})
        .to_string(),
    )
    .unwrap();
    for name in ["plug", "plugin:demo:server"] {
        settings
            .mcp_disable
            .insert(task.repo.clone(), vec!["global".into(), name.into()]);
        let tools = task_tools(&app, &settings, &task, dir.path());
        assert_eq!(
            tools
                .connected
                .servers
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["local"]
        );
        assert!(tools.task.is_empty());
        assert!(tools.notes.is_empty());
        assert!(tools.allowed().iter().all(|tool| !tool.contains("global")));
        assert_eq!(tools.codex_servers(Some(&json!({}))).len(), 1);
    }
}
