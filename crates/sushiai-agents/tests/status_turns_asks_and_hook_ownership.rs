//! Status turn scoping and ask matching, which hook entries are ours in a Codex hooks file, and
//! the launch spec the daemon keeps for itself.

use std::fs;

use serde_json::{json, Value};
use sushiai_agents::codex_hooks::{install, is_ours, merge, remove, uninstall};
use sushiai_agents::hooks::parse;
use sushiai_agents::launch::{
    build, claude_settings, permission_timeout, AgentSession, LaunchError, LaunchParams,
};
use sushiai_agents::status::{BlockedKind, ExitInfo, Input, SessionStatus, Status, StatusSource};
use sushiai_agents::Agent;

const BIN: &str = "/home/user/.sushiai/bin/sushiai";

fn h(seq: u64, json: &str) -> Input {
    Input::Hook {
        seq,
        payload: Box::new(parse(json.as_bytes()).unwrap()),
    }
}

fn fixture(agent: &str, name: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/{agent}/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read_to_string(p).unwrap()
}

fn ups(seq: u64, turn: &str) -> Input {
    h(
        seq,
        &format!(r#"{{"hook_event_name":"UserPromptSubmit","prompt_id":"{turn}"}}"#),
    )
}

fn ev(seq: u64, name: &str, turn: &str) -> Input {
    h(
        seq,
        &format!(r#"{{"hook_event_name":"{name}","prompt_id":"{turn}"}}"#),
    )
}

// P1.1
#[test]
fn clear_and_resume_session_end_is_not_an_exit() {
    let mut s = SessionStatus::new(0);
    s.apply(
        h(1, r#"{"hook_event_name":"SessionEnd","reason":"clear"}"#),
        1,
    );
    assert!(!s.ending);
    s.apply(
        h(
            2,
            r#"{"hook_event_name":"SessionStart","source":"clear","session_id":"n"}"#,
        ),
        2,
    );
    s.apply(Input::EndingTimeout, 9);
    assert_ne!(s.status, Status::Exited);
    // Even a real "other" end is cancelled by a later accepted hook.
    s.apply(
        h(3, r#"{"hook_event_name":"SessionEnd","reason":"other"}"#),
        3,
    );
    assert!(s.ending);
    s.apply(
        h(
            4,
            r#"{"hook_event_name":"SessionStart","source":"startup"}"#,
        ),
        4,
    );
    assert!(!s.ending);
    s.apply(Input::EndingTimeout, 9);
    assert_ne!(s.status, Status::Exited);
    s.apply(
        h(5, r#"{"hook_event_name":"SessionEnd","reason":"resume"}"#),
        5,
    );
    assert!(!s.ending);
}

// P1.2
#[test]
fn late_stop_of_an_earlier_turn_is_dropped() {
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t1"), 1);
    s.apply(ups(2, "t2"), 2);
    s.apply(ev(3, "Stop", "t1"), 3);
    assert_eq!(s.status, Status::Working);
    s.apply(ev(4, "PreToolUse", "t2"), 4);
    assert_eq!(s.status, Status::Working);
    s.apply(ev(5, "Stop", "t2"), 5);
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn late_permission_request_of_an_earlier_turn_is_dropped() {
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t1"), 1);
    s.apply(ups(2, "t2"), 2);
    s.apply(ev(3, "PermissionRequest", "t1"), 3);
    assert_eq!(s.status, Status::Working);
}

#[test]
fn codex_turn_id_scopes_too() {
    let mut s = SessionStatus::new(0);
    s.apply(
        h(1, r#"{"hook_event_name":"UserPromptSubmit","turn_id":"a"}"#),
        1,
    );
    s.apply(
        h(2, r#"{"hook_event_name":"UserPromptSubmit","turn_id":"b"}"#),
        2,
    );
    s.apply(h(3, r#"{"hook_event_name":"Stop","turn_id":"a"}"#), 3);
    assert_eq!(s.status, Status::Working);
}

// P1.3
fn ask(seq: u64, tool: &str, cmd: &str) -> Input {
    h(
        seq,
        &format!(
            r#"{{"hook_event_name":"PermissionRequest","tool_name":"{tool}","tool_input":{{"command":"{cmd}"}}}}"#
        ),
    )
}

fn post(seq: u64, name: &str, tool: &str, cmd: &str) -> Input {
    h(
        seq,
        &format!(
            r#"{{"hook_event_name":"{name}","tool_name":"{tool}","tool_input":{{"command":"{cmd}"}}}}"#
        ),
    )
}

#[test]
fn only_the_matching_post_tool_event_unblocks_an_ask() {
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t"), 1);
    s.apply(ask(2, "Bash", "rm x"), 2);
    // Another tool, or the same tool with another input, finished: still blocked.
    s.apply(post(3, "PostToolUse", "Read", "rm x"), 3);
    s.apply(post(4, "PostToolUse", "Bash", "ls"), 4);
    s.apply(post(5, "PreToolUse", "Bash", "rm x"), 5);
    assert_eq!(
        (s.status, s.blocked_kind),
        (Status::Blocked, Some(BlockedKind::Permission))
    );
    s.apply(post(6, "PostToolUseFailure", "Bash", "rm x"), 6);
    assert_eq!(s.status, Status::Working);
}

#[test]
fn real_codex_ask_is_closed_by_its_post_tool_use() {
    // The ask carries an extra `description`; the post event does not.
    let mut s = SessionStatus::new(0);
    s.apply(h(1, &fixture("codex", "UserPromptSubmit")), 1);
    s.apply(h(2, &fixture("codex", "PermissionRequest-bash")), 2);
    assert_eq!(s.status, Status::Blocked);
    s.apply(h(3, &fixture("codex", "PostToolUse-bash")), 3);
    assert_eq!(s.status, Status::Working);
}

// P1.4
#[test]
fn tool_events_clear_an_input_block() {
    for name in ["PreToolUse", "PostToolUse", "SubagentStart"] {
        let mut s = SessionStatus::new(0);
        s.apply(ups(1, "t"), 1);
        s.apply(
            h(
                2,
                r#"{"hook_event_name":"Notification","notification_type":"elicitation_dialog"}"#,
            ),
            2,
        );
        assert_eq!(s.blocked_kind, Some(BlockedKind::Input));
        s.apply(ev(3, name, "t"), 3);
        assert_eq!(s.status, Status::Working, "{name}");
    }
}

// P2.5
#[test]
fn handed_back_ask_stays_blocked_and_a_notification_keeps_it() {
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t"), 1);
    s.apply(ask(2, "Bash", "rm x"), 2);
    assert!(!s.apply(Input::PermissionClosed { decided: false }, 3));
    assert_eq!(s.status, Status::Blocked);
    s.apply(
        h(
            3,
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
        ),
        4,
    );
    assert_eq!(s.blocked_kind, Some(BlockedKind::Permission));
    // The tool finishing still closes it.
    s.apply(post(4, "PostToolUse", "Bash", "rm x"), 5);
    assert_eq!(s.status, Status::Working);
}

// P2.6
#[test]
fn subagent_post_tool_event_closes_a_pending_ask() {
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t"), 1);
    s.apply(
        h(
            2,
            r#"{"hook_event_name":"PermissionRequest","agent_id":"a","tool_name":"Bash","tool_input":{"command":"x"}}"#,
        ),
        2,
    );
    assert_eq!(s.status, Status::Blocked);
    s.apply(
        h(
            3,
            r#"{"hook_event_name":"PostToolUse","agent_id":"a","tool_name":"Bash","tool_input":{"command":"x"}}"#,
        ),
        3,
    );
    assert_eq!(s.status, Status::Working);
    // Without a pending ask a subagent tool event is still ignored.
    let mut s = SessionStatus::new(0);
    s.apply(ev(1, "Stop", "t"), 1);
    s.apply(
        h(2, r#"{"hook_event_name":"PostToolUse","agent_id":"a"}"#),
        2,
    );
    assert_eq!(s.status, Status::Idle);
}

// P2.10
#[test]
fn seq_zero_is_accepted_once() {
    let mut s = SessionStatus::new(0);
    assert!(s.apply(ups(0, "t"), 1));
    assert_eq!(s.source, StatusSource::Hook);
    assert!(!s.apply(ev(0, "Stop", "t"), 2));
    assert_eq!(s.status, Status::Working);
}

#[test]
fn exit_still_wins() {
    let mut s = SessionStatus::new(0);
    s.apply(ask(1, "Bash", "x"), 1);
    s.apply(
        Input::Exit(ExitInfo {
            code: None,
            signal: Some(9),
        }),
        2,
    );
    assert_eq!(s.status, Status::Exited);
}

// P2.7
#[test]
fn only_sushiai_commands_are_ours() {
    assert!(is_ours("/x/sushiai hook stop"));
    assert!(is_ours("'/a b/sushiai' hook stop --agent codex"));
    assert!(is_ours("'/a'\\''b/sushiai' hook stop"));
    assert!(!is_ours("/x/not-sushiai hook stop"));
    assert!(!is_ours("/x/other-tool hook stop"));
    assert!(!is_ours("echo /x/sushiai hook stop"));
    assert!(!is_ours("/x/sushiai status"));
}

#[test]
fn user_entry_that_looks_like_ours_is_untouched() {
    let x = json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "/x/not-sushiai hook stop"}
    ]}]}});
    let merged = merge(&x, BIN, 600).unwrap();
    assert_eq!(merged["hooks"]["Stop"][0], x["hooks"]["Stop"][0]);
    assert_eq!(remove(&merged).unwrap(), x);
}

#[test]
fn merge_twice_with_a_quoted_path_is_one_group() {
    let bin = "/home/o'brien/my dir/sushiai";
    let once = merge(&json!({}), bin, 600).unwrap();
    assert_eq!(merge(&once, bin, 600).unwrap(), once);
    assert_eq!(once["hooks"]["Stop"].as_array().unwrap().len(), 1);
}

#[test]
fn merge_refuses_a_foreign_binary_name() {
    assert!(merge(&json!({}), "/x/other", 600).is_err());
}

// P2.8
#[cfg(unix)]
#[test]
fn symlinked_hooks_file_is_written_through() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.json");
    let link = dir.path().join("hooks.json");
    fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    install(&link, BIN, 600, 1).unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::read_to_string(&target).unwrap().contains("sushiai"));
    uninstall(&link, 2).unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&target).unwrap()).unwrap(),
        json!({})
    );
}

// P2.9
#[test]
fn permission_timeout_has_one_owner() {
    assert_eq!(permission_timeout(600), 630);
    assert_eq!(permission_timeout(u64::MAX), u64::MAX);
    let c = claude_settings(BIN, 100).unwrap();
    assert_eq!(
        c["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
        130
    );
    let x = merge(&json!({}), BIN, 100).unwrap();
    assert_eq!(
        x["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
        130
    );
}

// P2.11
fn params<'a>(extra: &'a [String], session: AgentSession<'a>, agent: Agent) -> LaunchParams<'a> {
    LaunchParams {
        agent,
        hook_bin: BIN,
        socket: "/s",
        sushiai_session_id: "i",
        session_token: "t",
        session,
        extra_args: extra,
        prompt: None,
        permission_wait_secs: 600,
        claude_settings: None,
    }
}

#[test]
fn extra_args_cannot_override_daemon_flags() {
    for bad in [
        "--settings",
        "--settings={}",
        "--bare",
        "--session-id",
        "--session-id=x",
        "--resume",
        "-r",
    ] {
        let extra = vec![bad.to_owned()];
        let err = build(&params(&extra, AgentSession::New("u"), Agent::Claude)).unwrap_err();
        assert_eq!(err, LaunchError::ForbiddenArg(bad.to_owned()), "{bad}");
    }
}

#[test]
fn a_prompt_is_always_preceded_by_double_dash() {
    for agent in [Agent::Claude, Agent::Codex] {
        let mut p = params(&[], AgentSession::New("u"), agent);
        p.prompt = Some("hello");
        let argv = build(&p).unwrap().argv;
        let n = argv.len();
        assert_eq!(&argv[n - 2..], ["--", "hello"]);
    }
}

// P3
#[test]
fn session_ids_are_validated() {
    for bad in ["", "-x", "a b", "a;b", "--settings"] {
        let r = build(&params(&[], AgentSession::Resume(bad), Agent::Claude));
        assert_eq!(r.unwrap_err(), LaunchError::BadSessionId(bad.to_owned()));
        let r = build(&params(&[], AgentSession::Resume(bad), Agent::Codex));
        assert!(r.is_err(), "{bad}");
    }
    assert!(build(&params(
        &[],
        AgentSession::Resume("0a1b2c3d-1111-2222-3333-444455556666"),
        Agent::Codex
    ))
    .is_ok());
    // A new Codex session ignores the id.
    assert!(build(&params(&[], AgentSession::New(""), Agent::Codex)).is_ok());
}

#[test]
fn remove_prunes_only_groups_that_held_ours() {
    let x = json!({"hooks": {"Stop": [{"hooks": []}], "Custom": []}});
    assert_eq!(remove(&x).unwrap(), x);
    let m = merge(&x, BIN, 600).unwrap();
    assert_eq!(remove(&m).unwrap(), x);
}

#[test]
fn empty_file_becomes_a_clean_object_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    fs::write(&path, "  \n").unwrap();
    assert!(install(&path, BIN, 600, 1).unwrap().changed);
    uninstall(&path, 2).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
        json!({})
    );
}

#[test]
fn only_the_newest_three_backups_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    fs::write(&path, "{}").unwrap();
    for ts in 1..=6u64 {
        // Alternate so each call changes the file.
        if ts % 2 == 1 {
            install(&path, BIN, 600, ts).unwrap();
        } else {
            uninstall(&path, ts).unwrap();
        }
    }
    fs::write(dir.path().join("hooks.json.bak"), "keep").unwrap();
    let mut baks: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("hooks.json.bak-"))
        .collect();
    baks.sort();
    assert_eq!(
        baks,
        ["hooks.json.bak-4", "hooks.json.bak-5", "hooks.json.bak-6"]
    );
    assert!(dir.path().join("hooks.json.bak").exists());
}

#[test]
fn subagent_start_is_installed_for_claude() {
    let c = claude_settings(BIN, 600).unwrap();
    assert_eq!(
        c["hooks"]["SubagentStart"][0]["hooks"][0]["command"],
        format!("{BIN} hook subagent-start")
    );
}

#[test]
fn user_key_order_survives_a_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    fs::write(&path, r#"{"zeta": 1, "hooks": {}, "alpha": 2}"#).unwrap();
    install(&path, BIN, 600, 1).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let (z, h, a) = (
        text.find("zeta").unwrap(),
        text.find("\"hooks\"").unwrap(),
        text.find("alpha").unwrap(),
    );
    assert!(z < h && h < a, "{text}");
}

#[test]
fn ask_without_a_stored_input_is_cleared_by_any_post_tool_event() {
    // Blocked by a notification only (no PermissionRequest hook reached us).
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t"), 1);
    s.apply(
        h(
            2,
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
        ),
        2,
    );
    assert_eq!(s.status, Status::Blocked);
    s.apply(post(3, "PostToolUse", "Bash", "ls"), 3);
    assert_eq!(s.status, Status::Working);
    // A PermissionRequest without tool_input behaves the same.
    let mut s = SessionStatus::new(0);
    s.apply(
        h(
            1,
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash"}"#,
        ),
        1,
    );
    s.apply(post(2, "PostToolUse", "Bash", "ls"), 2);
    assert_eq!(s.status, Status::Working);
    // A screen-detected block too.
    let mut s = SessionStatus::new(0);
    s.apply(
        Input::Screen(sushiai_agents::status::Detected::Blocked(
            BlockedKind::Permission,
        )),
        1,
    );
    s.apply(Input::HookSilence, 3);
    s.apply(post(2, "PostToolUse", "Bash", "ls"), 4);
    assert_eq!(s.status, Status::Working);
}

#[test]
fn a_lost_prompt_hook_does_not_hide_the_next_turn() {
    let mut s = SessionStatus::new(0);
    s.apply(ups(1, "t1"), 1);
    s.apply(ev(2, "Stop", "t1"), 2);
    s.apply(ev(3, "PermissionRequest", "t2"), 3);
    assert_eq!(s.status, Status::Blocked);
    // The old turn's late events are still dropped.
    s.apply(ev(4, "Stop", "t1"), 4);
    assert_eq!(s.status, Status::Blocked);
    s.apply(ev(5, "Stop", "t2"), 5);
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn key_order_survives_remove() {
    let x = json!({"zeta": 1, "hooks": {"Stop": [{"hooks": [{"command": "s"}]}], "B": [{"hooks": [{"command": "u"}]}]}, "alpha": 2});
    let m = merge(&x, BIN, 600).unwrap();
    let r = remove(&m).unwrap();
    let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["zeta", "hooks", "alpha"]);
    let ev: Vec<&String> = r["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(ev, ["Stop", "B"]);
}
