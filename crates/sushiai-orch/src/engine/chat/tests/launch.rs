use super::ChatInvocation;
use crate::engine::chat::types::ChatMode;
use crate::engine::chat_tools;
use crate::policy::routing::Harness;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use serde_json::json;
use std::path::Path;

use super::{claude_argv, codex_argv, mcp_server, orchestrator_route, SERVER};
use crate::engine::chat::test_support::{route, test_app};

#[test]
fn claude_chat_reads_only_and_reaches_orchd_through_one_allowed_tools_value() {
    let argv = claude_argv(
        &ChatInvocation {
            route: &route(Harness::Claude),
            session: Some("sess"),
            mode: ChatMode::Chat,
            read_only: false,
        },
        Path::new("/d/mcp.json"),
        Path::new("/d/settings.json"),
        &chat_tools::TurnTools::default(),
    );
    let at = |flag: &str| argv[argv.iter().position(|a| a == flag).unwrap() + 1].clone();
    assert_eq!(at("--tools"), "Read,Grep,Glob");
    assert_eq!(at("--mcp-config"), "/d/mcp.json");
    assert_eq!(at("--resume"), "sess");
    assert_eq!(
        at("--allowedTools").split(',').count(),
        crate::api::mcp::ORCHESTRATOR_TOOLS.len()
    );
    assert!(at("--allowedTools").contains("mcp__sushiai-orchestrator__orchestrator_reply"));
    assert!(at("--allowedTools").contains("mcp__sushiai-orchestrator__task_delete"));
    assert_eq!(
        at("--append-system-prompt"),
        crate::prompts::default("chat")
    );
    assert!(!argv.iter().any(|a| a == "acceptEdits"));
}

#[test]
fn codex_chat_is_read_only_fresh_or_resumed() {
    let server = json!({"command": "/o", "args": ["mcp"], "env": {"ORCHD_TASK_MCP": "/f"}});
    let fresh = codex_argv(
        &ChatInvocation {
            route: &route(Harness::Codex),
            session: None,
            mode: ChatMode::Chat,
            read_only: true,
        },
        "/repo",
        &server,
        &chat_tools::TurnTools::default(),
    );
    assert_eq!(&fresh[..2], &["exec".to_string(), "--json".to_string()]);
    assert!(fresh
        .windows(2)
        .any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
    assert!(
        fresh.contains(&"mcp_servers.sushiai-orchestrator.env.ORCHD_TASK_MCP=\"/f\"".to_string())
    );
    let resumed = codex_argv(
        &ChatInvocation {
            route: &route(Harness::Codex),
            session: Some("t1"),
            mode: ChatMode::Chat,
            read_only: true,
        },
        "/repo",
        &server,
        &chat_tools::TurnTools::default(),
    );
    assert_eq!(
        &resumed[..3],
        &["exec".to_string(), "resume".to_string(), "t1".to_string()]
    );
    assert!(resumed.contains(&"sandbox_mode=\"read-only\"".to_string()));
    assert!(!resumed.contains(&"-C".to_string()));
    assert_eq!(resumed.last().unwrap(), "-");
}

#[test]
fn the_orchestrator_uses_its_own_route_else_the_standard_one() {
    let mut settings = Settings::default();
    let standard = settings.tiers.get(&Tier::Standard).unwrap().clone();
    assert_eq!(orchestrator_route(&settings).unwrap().id, standard);
    settings.orchestrator = "claude-opus".into();
    assert_eq!(orchestrator_route(&settings).unwrap().id, "claude-opus");
    settings.orchestrator = "gone".into();
    assert_eq!(orchestrator_route(&settings).unwrap().id, standard);
}

#[test]
fn both_prompts_say_to_name_tasks_by_title_and_the_draft_still_takes_ids() {
    for prompt in [
        crate::prompts::default("chat"),
        crate::prompts::default("brainstorm"),
        crate::prompts::default("plan"),
    ] {
        assert!(prompt.contains("refer to a task by its title, never by its id"));
        assert!(prompt.contains("'8ec68f8c'"));
    }
    assert!(crate::prompts::default("chat").contains("dependsOn"));
    assert!(crate::prompts::default("chat").contains("only the owner can decide"));
    assert!(crate::prompts::default("plan").contains("language of the owner's latest message"));
    assert!(crate::prompts::default("chat").contains("\"dependsOn\": [\"task ids\"]"));
}

#[test]
fn brainstorm_and_plan_turns_carry_their_prompt_and_only_read_only_tools() {
    for (mode, prompt) in [
        (ChatMode::Brainstorm, crate::prompts::default("brainstorm")),
        (ChatMode::Plan, crate::prompts::default("plan")),
    ] {
        assert_ne!(prompt, crate::prompts::default("chat"));
        for session in [None, Some("sess")] {
            let claude = claude_argv(
                &ChatInvocation {
                    route: &route(Harness::Claude),
                    session,
                    mode,
                    read_only: mode.read_only(),
                },
                Path::new("/d/mcp.json"),
                Path::new("/d/settings.json"),
                &chat_tools::TurnTools::default(),
            );
            let at =
                |flag: &str| claude[claude.iter().position(|a| a == flag).unwrap() + 1].clone();
            assert_eq!(at("--append-system-prompt"), prompt);
            let expected: Vec<String> = crate::api::mcp::READ_ONLY_TOOLS
                .iter()
                .map(|t| format!("mcp__{SERVER}__{t}"))
                .collect();
            assert_eq!(
                at("--allowedTools").split(',').collect::<Vec<_>>(),
                expected
            );
            assert_eq!(claude.contains(&"--resume".to_string()), session.is_some());

            let server = json!({"command": "/o", "args": ["mcp", "--read-only"]});
            let codex = codex_argv(
                &ChatInvocation {
                    route: &route(Harness::Codex),
                    session,
                    mode,
                    read_only: true,
                },
                "/repo",
                &server,
                &chat_tools::TurnTools::default(),
            );
            assert!(codex.contains(&format!("developer_instructions={}", json!(prompt))));
            assert!(!codex.contains(&format!(
                "developer_instructions={}",
                json!(crate::prompts::default("chat"))
            )));
        }
    }
}

#[test]
fn only_the_chat_mode_gets_a_writing_bridge() {
    let (app, _dir, _repo) = test_app();
    let has_read_only = |mode: ChatMode| {
        mcp_server(&app, None, mode.read_only(), false)["args"]
            .as_array()
            .unwrap()
            .contains(&json!("--read-only"))
    };
    assert!(!has_read_only(ChatMode::Chat));
    assert!(has_read_only(ChatMode::Brainstorm));
    assert!(has_read_only(ChatMode::Plan));
}

#[test]
fn the_mode_prompts_say_what_they_are_for() {
    for needle in [
        "exactly one focused question",
        "only the owner can decide",
        "language of the owner's latest message",
        "2-4 answer options",
        "weigh 2-3 approaches",
        "never create, start, stop, amend or answer tasks",
        "\"tasks\"",
        "dependsOn",
    ] {
        assert!(
            crate::prompts::default("brainstorm").contains(needle),
            "{needle}"
        );
    }
    for needle in [
        "small, ordered tasks",
        "how each is checked",
        "tier",
        "never create, start, stop, amend or answer tasks",
        "\"tasks\"",
    ] {
        assert!(crate::prompts::default("plan").contains(needle), "{needle}");
    }
}

#[test]
fn the_chat_mcp_config_declares_chat_or_overseer() {
    let (app, _dir, _) = test_app();
    for read_only in [false, true] {
        assert_eq!(
            mcp_server(&app, None, read_only, false)["env"]["ORCHD_BRIDGE_SOURCE"],
            "chat"
        );
        assert_eq!(
            mcp_server(&app, None, read_only, true)["env"]["ORCHD_BRIDGE_SOURCE"],
            "overseer"
        );
    }
}
