use sushiai_agents::hooks::{parse, HookPayload};
use sushiai_agents::status::{
    BlockedKind, Detected, ExitInfo, Input, SessionStatus, Status, StatusSource,
};

fn ev(json: &str) -> HookPayload {
    parse(json.as_bytes()).unwrap()
}

fn hook(seq: u64, name: &str) -> Input {
    Input::Hook {
        seq,
        payload: Box::new(ev(&format!(
            r#"{{"hook_event_name":"{name}","session_id":"s1","transcript_path":"/t/s1.jsonl"}}"#
        ))),
    }
}

fn hook_json(seq: u64, json: &str) -> Input {
    Input::Hook {
        seq,
        payload: Box::new(ev(json)),
    }
}

fn perm(seq: u64) -> Input {
    hook(seq, "PermissionRequest")
}

fn fresh() -> SessionStatus {
    SessionStatus::new(100)
}

#[test]
fn starts_in_starting_and_session_start_goes_idle_with_ids() {
    let mut s = fresh();
    assert_eq!(s.status, Status::Starting);
    assert!(s.apply(hook(1, "SessionStart"), 110));
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.source, StatusSource::Hook);
    assert_eq!(s.since, 110);
    assert_eq!(s.agent_session_id.as_deref(), Some("s1"));
    assert_eq!(s.transcript_path.as_deref(), Some("/t/s1.jsonl"));
}

#[test]
fn turn_cycle() {
    let mut s = fresh();
    s.apply(hook(1, "SessionStart"), 1);
    s.apply(hook(2, "UserPromptSubmit"), 2);
    assert_eq!(s.status, Status::Working);
    s.apply(hook(3, "PreToolUse"), 3);
    s.apply(hook(4, "PostToolUse"), 4);
    assert_eq!((s.status, s.since), (Status::Working, 2));
    s.apply(hook(5, "Stop"), 5);
    assert_eq!((s.status, s.since), (Status::Idle, 5));
}

#[test]
fn late_tool_event_after_stop_does_not_flip_to_working() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(hook(3, "Stop"), 3);
    s.apply(hook(4, "PostToolUse"), 4);
    assert_eq!(s.status, Status::Idle);
    s.apply(hook(5, "UserPromptSubmit"), 5);
    assert_eq!(s.status, Status::Working);
}

#[test]
fn stale_seq_is_ignored() {
    let mut s = fresh();
    s.apply(hook(5, "UserPromptSubmit"), 5);
    assert!(!s.apply(hook(4, "Stop"), 6));
    assert!(!s.apply(hook(5, "Stop"), 6));
    assert_eq!(s.status, Status::Working);
}

#[test]
fn permission_blocks_until_tool_finishes() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(perm(2), 2);
    assert_eq!(
        (s.status, s.blocked_kind),
        (Status::Blocked, Some(BlockedKind::Permission))
    );
    // An async PreToolUse that arrives late must not clear the ask.
    s.apply(hook(3, "PreToolUse"), 3);
    assert_eq!(s.status, Status::Blocked);
    s.apply(hook(4, "PostToolUse"), 4);
    assert_eq!(
        (s.status, s.blocked_kind, s.since),
        (Status::Working, None, 4)
    );
}

#[test]
fn permission_closed_unblocks_and_stop_ends_the_turn() {
    let mut s = fresh();
    s.apply(perm(1), 1);
    assert!(s.apply(Input::PermissionClosed { decided: true }, 2));
    assert_eq!(s.status, Status::Working);
    s.apply(perm(2), 3);
    s.apply(hook(3, "Stop"), 4);
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn permission_closed_does_not_touch_other_states() {
    let mut s = fresh();
    s.apply(hook(1, "Stop"), 1);
    assert!(!s.apply(Input::PermissionClosed { decided: true }, 2));
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn notifications() {
    let note = |seq, t: &str| {
        hook_json(
            seq,
            &format!(r#"{{"hook_event_name":"Notification","notification_type":"{t}"}}"#),
        )
    };
    // permission_prompt alone blocks (no PermissionRequest hook this turn).
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(note(2, "permission_prompt"), 2);
    assert_eq!(
        (s.status, s.blocked_kind),
        (Status::Blocked, Some(BlockedKind::Permission))
    );
    // After a PermissionRequest was answered, a late permission_prompt is ignored.
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(perm(2), 2);
    s.apply(hook(3, "PostToolUse"), 3);
    s.apply(note(4, "permission_prompt"), 4);
    assert_eq!(s.status, Status::Working);
    // Input dialogs block with kind input; idle_prompt idles a working session.
    s.apply(note(5, "elicitation_dialog"), 5);
    assert_eq!(
        (s.status, s.blocked_kind),
        (Status::Blocked, Some(BlockedKind::Input))
    );
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(note(2, "idle_prompt"), 2);
    assert_eq!(s.status, Status::Idle);
    s.apply(note(3, "auth_success"), 3);
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn stop_failure_idles_with_error_flag_cleared_by_next_prompt() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(hook(2, "StopFailure"), 2);
    assert_eq!(s.status, Status::Idle);
    assert!(s.error);
    s.apply(hook(3, "UserPromptSubmit"), 3);
    assert!(!s.error);
}

#[test]
fn subagent_events_change_nothing_except_a_permission_ask() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(
        hook_json(2, r#"{"hook_event_name":"Stop","agent_id":"a1"}"#),
        2,
    );
    assert_eq!(s.status, Status::Working);
    s.apply(
        hook_json(
            3,
            r#"{"hook_event_name":"PermissionRequest","agent_id":"a1"}"#,
        ),
        3,
    );
    assert_eq!(s.status, Status::Blocked);
}

#[test]
fn session_start_again_replaces_id_and_keeps_state() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(
        hook_json(
            2,
            r#"{"hook_event_name":"SessionStart","session_id":"s2","source":"clear"}"#,
        ),
        2,
    );
    assert_eq!(s.status, Status::Working);
    assert_eq!(s.agent_session_id.as_deref(), Some("s2"));
}

#[test]
fn unknown_event_does_not_change_state_or_source() {
    let mut s = fresh();
    assert!(!s.apply(hook(1, "SomethingNew"), 1));
    assert_eq!(
        (s.status, s.source),
        (Status::Starting, StatusSource::Heuristic)
    );
}

#[test]
fn exit_is_authoritative_and_terminal() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    assert!(s.apply(
        Input::Exit(ExitInfo {
            code: Some(0),
            signal: None
        }),
        9
    ));
    assert_eq!((s.status, s.since), (Status::Exited, 9));
    // Late events after exit are ignored, including a second exit.
    for input in [
        hook(2, "UserPromptSubmit"),
        perm(3),
        Input::Screen(Detected::Working),
        Input::Exit(ExitInfo {
            code: Some(1),
            signal: None,
        }),
        Input::EndingTimeout,
    ] {
        assert!(!s.apply(input, 20));
    }
    assert_eq!(
        s.exit,
        Some(ExitInfo {
            code: Some(0),
            signal: None
        })
    );
    assert_eq!(s.since, 9);
}

#[test]
fn session_end_waits_for_pty_exit_or_timeout() {
    let mut s = fresh();
    s.apply(hook(1, "UserPromptSubmit"), 1);
    s.apply(hook(2, "SessionEnd"), 2);
    assert!(s.ending);
    assert_eq!(s.status, Status::Working);
    s.apply(Input::EndingTimeout, 5);
    assert_eq!((s.status, s.since), (Status::Exited, 5));
    // Without a SessionEnd the timeout does nothing.
    let mut s = fresh();
    assert!(!s.apply(Input::EndingTimeout, 5));
}

#[test]
fn screen_applies_only_while_source_is_heuristic() {
    let mut s = fresh();
    assert!(s.apply(Input::Screen(Detected::Idle), 1));
    assert_eq!(s.status, Status::Idle);
    s.apply(Input::Screen(Detected::Blocked(BlockedKind::Input)), 2);
    assert_eq!(
        (s.status, s.blocked_kind),
        (Status::Blocked, Some(BlockedKind::Input))
    );
    // First hook flips the source; screen is then ignored.
    s.apply(hook(1, "UserPromptSubmit"), 3);
    assert_eq!(s.source, StatusSource::Hook);
    assert!(!s.apply(Input::Screen(Detected::Idle), 4));
    // Hook silence hands control back to the screen.
    s.apply(Input::HookSilence, 5);
    assert_eq!(s.source, StatusSource::Heuristic);
    s.apply(Input::Screen(Detected::Idle), 6);
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn repeated_same_state_keeps_since() {
    let mut s = fresh();
    s.apply(Input::Screen(Detected::Working), 10);
    assert!(!s.apply(Input::Screen(Detected::Working), 20));
    assert_eq!(s.since, 10);
}

#[test]
fn session_start_ends_a_dialog_read_from_the_screen_before_any_hook() {
    let mut s = fresh();
    s.apply(Input::Screen(Detected::Blocked(BlockedKind::Input)), 1);
    s.apply(hook(1, "SessionStart"), 2);
    assert_eq!((s.status, s.source), (Status::Idle, StatusSource::Hook));
    // A dialog the hooks reported stays: only a screen-read one ends at SessionStart.
    let mut p = fresh();
    p.apply(hook(1, "UserPromptSubmit"), 1);
    p.apply(Input::HookSilence, 2);
    p.apply(Input::Screen(Detected::Blocked(BlockedKind::Permission)), 3);
    p.apply(hook(2, "SessionStart"), 4);
    assert_eq!(p.status, Status::Blocked);
}
