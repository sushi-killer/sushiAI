use super::base_req;
use crate::harness::{
    build_argv, build_argv_with_schema, claude_argv, codex_argv, codex_mcp_flags, prompt_hash,
    OutputSchema, RunRequest, DELEGATION_TOOLS,
};
use crate::policy::routing::Harness;
use crate::sandbox::TaskSandbox;
use std::path::{Path, PathBuf};

#[test]
fn codex_task_implements_use_full_access_only_with_the_outer_sandbox() {
    let wt = Path::new("/worktree");
    let mut req = base_req(Harness::Codex, wt);
    req.task_run = true;
    req.network_allowed = false;
    assert!(codex_argv(&req).contains(&"workspace-write".to_string()));

    let root = tempfile::tempdir().unwrap();
    let policy = TaskSandbox::build(crate::sandbox::Inputs {
        repo_root: root.path(),
        worktree: root.path(),
        run_dir: root.path(),
        data_dir: &root.path().join("data"),
        sushiai_home: &root.path().join("sushiai"),
        worktree_root: "",
        home: &root.path().join("home"),
        daemon_socket: &root.path().join("daemon.sock"),
        cargo_home: None,
        rustup_home: None,
        tmp_dir: None,
        npm_cache: None,
        known_project_folders: &[],
        known_task_worktrees: &[],
    });
    req.task_sandbox = Some(&policy);
    let mode = if cfg!(target_os = "macos") {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    assert!(codex_argv(&req).contains(&mode.to_string()));
    req.resume = Some("session");
    assert!(codex_argv(&req).contains(&format!("sandbox_mode=\"{mode}\"")));
    req.review = true;
    assert!(codex_argv(&req).contains(&"sandbox_mode=\"read-only\"".to_string()));
    req.resume = None;
    assert!(codex_argv(&req).contains(&"read-only".to_string()));
}

#[test]
fn native_claude_tasks_cannot_inherit_project_sandbox_grants() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let policy = TaskSandbox::build(crate::sandbox::Inputs {
        repo_root: root,
        worktree: root,
        run_dir: root,
        data_dir: &root.join("data"),
        sushiai_home: &root.join("sushiai"),
        worktree_root: "",
        home: &root.join("home"),
        daemon_socket: &root.join("daemon.sock"),
        cargo_home: None,
        rustup_home: None,
        tmp_dir: None,
        npm_cache: None,
        known_project_folders: &[],
        known_task_worktrees: &[],
    });
    let mut req = base_req(Harness::Claude, root);
    req.repo_settings = true;
    req.task_run = true;
    req.task_sandbox = Some(&policy);
    let sources = |req: &RunRequest| {
        let argv = claude_argv(req);
        argv[argv.iter().position(|a| a == "--setting-sources").unwrap() + 1].clone()
    };
    assert_eq!(sources(&req), "");
    req.task_sandbox = None;
    assert_eq!(sources(&req), "project,local");
    req.task_sandbox = Some(&policy);
    req.task_run = false;
    assert_eq!(sources(&req), "project,local");
}

#[test]
fn a_resumed_run_carries_the_session_and_keeps_its_prompt_hash() {
    let wt = PathBuf::from("/repo-task");
    let claude = base_req(Harness::Claude, &wt);
    let mut resumed = base_req(Harness::Claude, &wt);
    resumed.resume = Some("sess-1");
    let argv = claude_argv(&resumed);
    let at = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[at + 1], "sess-1");
    assert_eq!(
        prompt_hash(&argv, None),
        prompt_hash(&claude_argv(&claude), None)
    );

    let mut codex = base_req(Harness::Codex, &wt);
    codex.review = true;
    codex.resume = Some("sess-1");
    let argv = codex_argv(&codex);
    assert_eq!(&argv[..3], ["exec", "resume", "sess-1"]);
    assert!(
        !argv.iter().any(|a| a == "-C" || a == "--sandbox"),
        "{argv:?}"
    );
    assert!(argv.contains(&"sandbox_mode=\"read-only\"".to_string()));
    assert_eq!(argv.last().map(String::as_str), Some("-"));
    codex.resume = Some("sess-2");
    assert_eq!(
        prompt_hash(&argv, None),
        prompt_hash(&codex_argv(&codex), None)
    );
    codex.review = false;
    for network in [false, true] {
        codex.network_allowed = network;
        let argv = codex_argv(&codex);
        assert_eq!(
            &argv[..5],
            [
                "exec",
                "resume",
                "sess-2",
                "--json",
                "--skip-git-repo-check"
            ]
        );
        assert!(!argv.iter().any(|a| a == "-C" || a == "--sandbox"));
        assert!(argv
            .windows(2)
            .any(|w| w == ["-c", "sandbox_mode=\"workspace-write\""]));
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-c"
                && w[1] == format!("sandbox_workspace_write.network_access={network}")));
    }
}

#[test]
fn codex_gets_a_task_server_and_an_http_one_as_a_url() {
    let wt = PathBuf::from("/repo-task");
    let mut req = base_req(Harness::Codex, &wt);
    let servers = [
        (
            "design".to_string(),
            serde_json::json!({"command": "npx", "args": ["-y", "d"], "env": {"K": "v"}}),
        ),
        (
            "web".to_string(),
            serde_json::json!({"type": "http", "url": "https://example.test/mcp"}),
        ),
    ];
    req.codex_mcp = &servers;
    let argv = codex_argv(&req);
    for flag in [
        "mcp_servers.design.command=\"npx\"",
        "mcp_servers.design.args=[\"-y\",\"d\"]",
        "mcp_servers.design.env.K=\"v\"",
        "mcp_servers.web.url=\"https://example.test/mcp\"",
    ] {
        assert!(argv.contains(&flag.to_string()), "{flag}: {argv:?}");
    }
    assert!(!argv
        .iter()
        .any(|a| a.starts_with("mcp_servers.web.command")));
}

#[test]
fn a_definition_codex_cannot_take_gets_no_flags() {
    use serde_json::json;
    assert_eq!(
        codex_mcp_flags("bare", &json!({"command": "x"})),
        vec!["-c", "mcp_servers.bare.command=\"x\""]
    );
    for (name, def) in [
        ("a.b", json!({"command": "x"})),
        ("a b", json!({"command": "x"})),
        ("none", json!({"args": ["y"]})),
        (
            "auth",
            json!({"url": "https://e.test", "headers": {"Authorization": "t"}}),
        ),
        ("sse", json!({"type": "sse", "url": "https://e.test"})),
    ] {
        assert!(codex_mcp_flags(name, &def).is_empty(), "{name}");
    }
    let vars = json!({"command": "x", "env": {"A.B": "1", "OK": "2"}});
    let flags = codex_mcp_flags("e", &vars);
    assert!(flags.contains(&"mcp_servers.e.env.OK=\"2\"".to_string()));
    assert!(!flags.iter().any(|f| f.contains("A.B")));
}

#[test]
fn codex_gets_the_messages_server() {
    let wt = PathBuf::from("/repo-task");
    let server = serde_json::json!({"command": "/o", "args": ["mcp", "--task", "t"]});
    let mut req = base_req(Harness::Codex, &wt);
    let servers = [("sushiai-messages".to_string(), server.clone())];
    req.codex_mcp = &servers;
    let command = "mcp_servers.sushiai-messages.command=\"/o\"".to_string();
    let args = "mcp_servers.sushiai-messages.args=[\"mcp\",\"--task\",\"t\"]".to_string();
    let argv = codex_argv(&req);
    assert!(argv.contains(&command));
    assert!(argv.contains(&args));
    assert_eq!(argv.last().unwrap(), "-");
}

#[test]
fn effort_and_the_attempt_cap_reach_every_argv_shape_or_none() {
    let wt = PathBuf::from("/repo-task");
    for harness in [Harness::Claude, Harness::Codex] {
        for review in [false, true] {
            let mut req = base_req(harness, &wt);
            req.review = review;
            let none = build_argv(&req);
            assert!(!none.iter().any(|a| a.contains("effort")), "{none:?}");
            assert!(!none.contains(&"--max-budget-usd".to_string()));
            req.effort = Some("medium");
            req.max_budget_usd = Some(1.5);
            let argv = build_argv(&req);
            match harness {
                Harness::Claude => {
                    let i = argv.iter().position(|a| a == "--effort").unwrap();
                    assert_eq!(argv[i + 1], "medium");
                    let i = argv.iter().position(|a| a == "--max-budget-usd").unwrap();
                    assert_eq!(argv[i + 1], "1.5");
                }
                Harness::Codex => {
                    assert!(argv.contains(&"model_reasoning_effort=\"medium\"".to_string()));
                    assert!(!argv.contains(&"--max-budget-usd".to_string()));
                }
            }
        }
    }
}

#[test]
fn claude_implement_argv_has_accept_edits_and_no_tools_restriction() {
    let wt = PathBuf::from("/repo-task");
    let mcp = PathBuf::from("/repo-task/runs/1/mcp.json");
    let settings = PathBuf::from("/repo-task/runs/1/settings.json");
    let mut req = base_req(Harness::Claude, &wt);
    req.mcp_config = Some(&mcp);
    req.settings_path = Some(&settings);
    req.model = Some("sonnet");
    let argv = claude_argv(&req);
    assert!(argv.contains(&"--permission-mode".to_string()));
    let idx = argv.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(argv[idx + 1], "acceptEdits");
    assert!(!argv.contains(&"--tools".to_string()));
    assert!(argv.contains(&"--mcp-config".to_string()));
    assert!(argv.contains(&"--model".to_string()));
}

/// Pins today's argv: an implement run always trims the delegation
/// tools now, unconditionally.
#[test]
fn claude_implement_argv_always_trims_delegation_tools() {
    let wt = PathBuf::from("/w");
    let mcp = PathBuf::from("/r/mcp.json");
    let settings = PathBuf::from("/r/settings.json");
    let mut req = base_req(Harness::Claude, &wt);
    req.mcp_config = Some(&mcp);
    req.settings_path = Some(&settings);
    req.model = Some("opus");
    req.effort = Some("high");
    let expected: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--setting-sources",
        "project,local",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--mcp-config",
        "/r/mcp.json",
        "--settings",
        "/r/settings.json",
        "--permission-mode",
        "acceptEdits",
        "--disallowedTools",
        DELEGATION_TOOLS,
        "--permission-prompts",
        "none",
        "--model",
        "opus",
        "--effort",
        "high",
    ]
    .map(String::from)
    .to_vec();
    let argv = claude_argv(&req);
    assert_eq!(argv, expected);
    let at = argv.iter().position(|a| a == "--disallowedTools").unwrap();
    assert!(argv[at + 1].split(',').any(|t| t == "Task"));
}

#[test]
fn claude_review_never_gets_the_delegation_tools_trim() {
    let wt = PathBuf::from("/w");
    let mut req = base_req(Harness::Claude, &wt);
    req.review = true;
    assert!(!claude_argv(&req).contains(&"--disallowedTools".to_string()));
}

#[test]
fn the_schema_flag_reaches_each_harness_and_its_file_is_not_hashed() {
    let wt = PathBuf::from("/repo-task");
    let req = base_req(Harness::Claude, &wt);
    let json = r#"{"type":"object"}"#;
    let (a, b) = (Path::new("/run/a.json"), Path::new("/run/b.json"));
    assert_eq!(build_argv(&req), claude_argv(&req));
    let argv = build_argv_with_schema(&req, Some(&OutputSchema { json, path: a }));
    let flag = argv.iter().position(|a| a == "--json-schema").unwrap();
    assert_eq!(argv[flag + 1], json);

    let req = base_req(Harness::Codex, &wt);
    let argv = build_argv_with_schema(&req, Some(&OutputSchema { json, path: a }));
    let flag = argv.iter().position(|a| a == "--output-schema").unwrap();
    assert_eq!(argv[flag + 1], "/run/a.json");
    assert_eq!(argv.last().map(String::as_str), Some("-"));
    let other = build_argv_with_schema(&req, Some(&OutputSchema { json, path: b }));
    assert_eq!(
        prompt_hash(&argv, None),
        prompt_hash(&other, None),
        "the per-run schema file is not part of the fingerprint"
    );
}

#[test]
fn claude_review_uses_read_only_tools_and_plan_mode() {
    let wt = PathBuf::from("/repo-task");
    let mut req = base_req(Harness::Claude, &wt);
    req.review = true;
    let argv = claude_argv(&req);
    assert!(argv.contains(&"--tools".to_string()));
    let tools_idx = argv.iter().position(|a| a == "--tools").unwrap();
    assert_eq!(argv[tools_idx + 1], "Read,Grep,Glob");
    let mode_idx = argv.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(argv[mode_idx + 1], "plan");
}

#[test]
fn codex_implement_argv_sets_workspace_write_and_network_flag() {
    let wt = PathBuf::from("/repo-task");
    let mut req = base_req(Harness::Codex, &wt);
    req.network_allowed = false;
    let argv = codex_argv(&req);
    assert!(argv.contains(&"workspace-write".to_string()));
    assert!(argv.contains(&"sandbox_workspace_write.network_access=false".to_string()));
    assert_eq!(argv.last().unwrap(), "-");
}

#[test]
fn codex_review_attaches_images_before_the_other_flags() {
    let images = vec![std::path::PathBuf::from("/w/artifacts/a.png")];
    let mut req = base_req(Harness::Codex, Path::new("/w"));
    req.review = true;
    req.images = &images;
    let argv = codex_argv(&req);
    assert_eq!(argv[1], "--image=/w/artifacts/a.png");
    assert_eq!(argv.last().unwrap(), "-");
}

#[test]
fn codex_review_is_sandboxed_read_only() {
    let wt = PathBuf::from("/repo-task");
    let mut req = base_req(Harness::Codex, &wt);
    req.review = true;
    let argv = codex_argv(&req);
    assert!(argv.contains(&"read-only".to_string()));
    assert!(!argv.iter().any(|a| a.contains("network_access")));
}
