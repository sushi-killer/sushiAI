//! Parser tests over payloads captured from the real CLIs (claude 2.1.291,
//! codex-cli 0.160.0). Paths and hostnames are scrubbed; shapes are untouched.

use sushiai_agents::hooks::{parse, HookEvent, ParseError};

fn fixture(agent: &str, name: &str) -> Vec<u8> {
    let p = format!(
        "{}/tests/fixtures/{agent}/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

#[test]
fn claude_session_start_startup_and_resume() {
    for (name, source) in [
        ("SessionStart-startup", "startup"),
        ("SessionStart-resume", "resume"),
    ] {
        let p = parse(&fixture("claude", name)).unwrap();
        assert_eq!(
            p.event,
            HookEvent::SessionStart {
                source: Some(source.into())
            }
        );
        assert_eq!(p.common.session_id.as_deref().map(str::len), Some(36));
        assert_eq!(p.common.cwd.as_deref(), Some("/home/user/proj"));
        let t = p.common.transcript_path.unwrap();
        assert!(t.ends_with(&format!("{}.jsonl", p.common.session_id.unwrap())));
    }
}

#[test]
fn claude_turn_and_tool_events() {
    let p = parse(&fixture("claude", "UserPromptSubmit")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::UserPromptSubmit {
            prompt: Some(
                "Run the shell command: touch probe.txt. Then reply with the word ok.".into()
            )
        }
    );
    assert_eq!(p.common.permission_mode.as_deref(), Some("auto"));
    let p = parse(&fixture("claude", "PreToolUse-bash")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::PreToolUse {
            tool_name: Some("Bash".into())
        }
    );
    let p = parse(&fixture("claude", "PostToolUse-bash")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::PostToolUse {
            tool_name: Some("Bash".into())
        }
    );
    assert_eq!(
        parse(&fixture("claude", "Stop")).unwrap().event,
        HookEvent::Stop
    );
    assert_eq!(
        parse(&fixture("claude", "SessionEnd")).unwrap().event,
        HookEvent::SessionEnd {
            reason: Some("other".into())
        }
    );
}

#[test]
fn claude_permission_request_keeps_tool_input() {
    let p = parse(&fixture("claude", "PermissionRequest-bash")).unwrap();
    let HookEvent::PermissionRequest(r) = p.event else {
        panic!("not a permission request");
    };
    assert_eq!(r.tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        r.tool_input["command"],
        "touch probe2.txt && ls /nonexistent_probe_dir"
    );
    assert_eq!(p.common.permission_mode.as_deref(), Some("default"));
    assert!(p.common.agent_id.is_none());
}

#[test]
fn codex_events() {
    let p = parse(&fixture("codex", "SessionStart-startup")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::SessionStart {
            source: Some("startup".into())
        }
    );
    assert!(p.common.transcript_path.unwrap().contains("rollout-"));
    assert!(parse(&fixture("codex", "UserPromptSubmit"))
        .unwrap()
        .common
        .turn_id
        .is_some());
    assert_eq!(
        parse(&fixture("codex", "PreToolUse-bash")).unwrap().event,
        HookEvent::PreToolUse {
            tool_name: Some("Bash".into())
        }
    );
    assert_eq!(
        parse(&fixture("codex", "PostToolUse-bash")).unwrap().event,
        HookEvent::PostToolUse {
            tool_name: Some("Bash".into())
        }
    );
    assert_eq!(
        parse(&fixture("codex", "Stop")).unwrap().event,
        HookEvent::Stop
    );
    assert_eq!(
        parse(&fixture("codex", "SessionEnd")).unwrap().event,
        HookEvent::SessionEnd {
            reason: Some("other".into())
        }
    );
    let HookEvent::PermissionRequest(r) = parse(&fixture("codex", "PermissionRequest-bash"))
        .unwrap()
        .event
    else {
        panic!("not a permission request");
    };
    assert_eq!(r.tool_input["command"], "touch probe.txt");
}

#[test]
fn unknown_event_and_fields_never_fail() {
    let p =
        parse(br#"{"hook_event_name":"BrandNewEvent","session_id":"s","extra":{"a":1}}"#).unwrap();
    assert_eq!(p.event, HookEvent::Unknown("BrandNewEvent".into()));
    assert_eq!(p.common.session_id.as_deref(), Some("s"));
    let p = parse(b"{}").unwrap();
    assert_eq!(p.event, HookEvent::Unknown(String::new()));
}

#[test]
fn null_and_wrong_typed_fields_are_none() {
    let p = parse(br#"{"hook_event_name":"Stop","transcript_path":null,"cwd":7}"#).unwrap();
    assert!(p.common.transcript_path.is_none());
    assert!(p.common.cwd.is_none());
}

#[test]
fn notification_and_subagent_fields() {
    let p = parse(
        br#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"m","agent_id":"a1"}"#,
    )
    .unwrap();
    assert_eq!(
        p.event,
        HookEvent::Notification {
            notification_type: Some("permission_prompt".into()),
            message: Some("m".into())
        }
    );
    assert_eq!(p.common.agent_id.as_deref(), Some("a1"));
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(matches!(parse(b""), Err(ParseError::Json(_))));
    assert!(matches!(parse(b"\xff\xfe"), Err(ParseError::Json(_))));
    assert!(matches!(parse(b"[1]"), Err(ParseError::NotAnObject)));
    assert!(matches!(parse(b"\"x\""), Err(ParseError::NotAnObject)));
}

#[test]
fn more_real_claude_events() {
    let p = parse(&fixture("claude", "SessionStart-clear")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::SessionStart {
            source: Some("clear".into())
        }
    );
    // `/clear` mints a new session id; the SessionEnd of the old one says why.
    let end = parse(&fixture("claude", "SessionEnd-clear")).unwrap();
    assert_eq!(
        end.event,
        HookEvent::SessionEnd {
            reason: Some("clear".into())
        }
    );
    assert_ne!(end.common.session_id, p.common.session_id);
    for (name, reason) in [
        ("SessionEnd-other", "other"),
        ("SessionEnd-prompt_input_exit", "prompt_input_exit"),
    ] {
        assert_eq!(
            parse(&fixture("claude", name)).unwrap().event,
            HookEvent::SessionEnd {
                reason: Some(reason.into())
            }
        );
    }
    let n = parse(&fixture("claude", "Notification-permission_prompt")).unwrap();
    assert_eq!(
        n.event,
        HookEvent::Notification {
            notification_type: Some("permission_prompt".into()),
            message: Some("Claude needs your permission".into())
        }
    );
    assert!(n.common.prompt_id.is_some());
    // Subagent events carry agent_id; the main-thread ones do not.
    for name in ["SubagentStart", "SubagentStop"] {
        let p = parse(&fixture("claude", name)).unwrap();
        assert!(p.common.agent_id.is_some(), "{name}");
    }
    assert_eq!(
        parse(&fixture("claude", "SubagentStart")).unwrap().event,
        HookEvent::SubagentStart
    );
    assert_eq!(
        parse(&fixture("claude", "SubagentStop")).unwrap().event,
        HookEvent::SubagentStop
    );
    let p = parse(&fixture("claude", "PreToolUse-agent")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::PreToolUse {
            tool_name: Some("Agent".into())
        }
    );
    assert!(p.common.tool_input.is_some());
}

#[test]
fn real_codex_resume_start() {
    let p = parse(&fixture("codex", "SessionStart-resume")).unwrap();
    assert_eq!(
        p.event,
        HookEvent::SessionStart {
            source: Some("resume".into())
        }
    );
    assert_eq!(
        p.common.session_id,
        parse(&fixture("codex", "SessionStart-startup"))
            .unwrap()
            .common
            .session_id
    );
}

/// Fields the parser reads but no captured payload has. Each is a guess that
/// must stay `Option`; the test fails when a fixture gains one, so the entry
/// gets removed.
const NOT_CAPTURED_YET: &[&str] = &["error"];

#[test]
fn every_field_the_parser_reads_appears_in_a_real_fixture() {
    let mut seen = std::collections::BTreeSet::new();
    for agent in ["claude", "codex"] {
        let dir = format!("{}/tests/fixtures/{agent}", env!("CARGO_MANIFEST_DIR"));
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "json") {
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                seen.extend(v.as_object().unwrap().keys().cloned());
            }
        }
    }
    for f in sushiai_agents::hooks::READ_FIELDS {
        let captured = seen.contains(*f);
        let guess = NOT_CAPTURED_YET.contains(f);
        assert!(
            captured != guess,
            "field {f}: captured={captured}, listed as not captured={guess}"
        );
    }
}

#[test]
fn prompt_title_is_the_first_line_cut_at_a_word() {
    use sushiai_agents::hooks::prompt_title;
    assert_eq!(
        prompt_title("  Fix the login form \n").as_deref(),
        Some("Fix the login form")
    );
    assert_eq!(
        prompt_title("\n\nAdd a retry\r\nthen tests").as_deref(),
        Some("Add a retry")
    );
    assert_eq!(prompt_title("  \n "), None);
    let long =
        prompt_title("Refactor the synthetic billing module so every invoice shares one formatter")
            .unwrap();
    assert_eq!(long, "Refactor the synthetic billing module…");
    let word = prompt_title(&"x".repeat(100)).unwrap();
    assert_eq!(word.chars().count(), 40);
    assert!(word.ends_with('…'));
    let wide = prompt_title(&"é".repeat(100)).unwrap();
    assert_eq!(wide.chars().count(), 40);
}

#[test]
fn prompt_title_skips_bracketed_notes_and_skill_tags() {
    use sushiai_agents::hooks::prompt_title;
    assert_eq!(
        prompt_title("[Client: synthetic note about the sender. Keep it short.]\nFix the form")
            .as_deref(),
        Some("Fix the form")
    );
    assert_eq!(
        prompt_title("[Image #1]\n[Image #2]\n\nDescribe these").as_deref(),
        Some("Describe these")
    );
    assert_eq!(prompt_title("[Image #1]"), None);
    assert_eq!(
        prompt_title("[not a note] but text").as_deref(),
        Some("[not a note] but text")
    );
    let skill = "<command-message>demo</command-message>\n<command-name>/demo</command-name>\n<command-args>tidy the synthetic notes</command-args>";
    assert_eq!(
        prompt_title(skill).as_deref(),
        Some("tidy the synthetic notes")
    );
    assert_eq!(
        prompt_title("<command-message>demo</command-message>\n<command-name>/demo</command-name>"),
        None
    );
}
