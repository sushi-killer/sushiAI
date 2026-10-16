use super::claude_stream;
use crate::harness::{exit_error, feed_stream_line, PermissionDenial, RunOutcome};
use crate::policy::routing::Harness;

#[test]
fn a_silent_failed_exit_reads_as_words_without_a_code() {
    use std::os::unix::process::ExitStatusExt;
    let exited = std::process::ExitStatus::from_raw(1 << 8);
    assert_eq!(
        exit_error(Harness::Claude, exited),
        "Claude exited with an error."
    );
    let killed = std::process::ExitStatus::from_raw(9);
    assert_eq!(
        exit_error(Harness::Codex, killed),
        "Codex was stopped by a signal."
    );
}

#[test]
fn a_structured_output_is_the_reply_and_replaces_the_prose_result() {
    let out = claude_stream(&[
        r#"{"type":"result","subtype":"success","is_error":false,"result":"Done.","structured_output":{"title":"t"}}"#,
    ]);
    assert_eq!(out.final_text.as_deref(), Some(r#"{"title":"t"}"#));
}

#[test]
fn feeds_claude_stream_lines_into_outcome() {
    let mut outcome = RunOutcome::default();
    feed_stream_line(
        Harness::Claude,
        r#"{"type":"system","subtype":"init","session_id":"sess-9"}"#,
        &mut outcome,
    );
    feed_stream_line(
        Harness::Claude,
        r#"{"type":"result","total_cost_usd":0.42,"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5},"result":"done"}"#,
        &mut outcome,
    );
    assert_eq!(outcome.session_id.as_deref(), Some("sess-9"));
    assert_eq!(outcome.cost_usd, Some(0.42));
    assert_eq!(outcome.usage_input, 10);
    assert_eq!(outcome.final_text.as_deref(), Some("done"));
}

#[test]
fn feeds_codex_stream_lines_into_outcome() {
    let mut outcome = RunOutcome::default();
    feed_stream_line(
        Harness::Codex,
        r#"{"type":"thread.started","thread_id":"th-1"}"#,
        &mut outcome,
    );
    feed_stream_line(
        Harness::Codex,
        r#"{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":4,"cached_input_tokens":1}}"#,
        &mut outcome,
    );
    feed_stream_line(
        Harness::Codex,
        r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"all set"}}"#,
        &mut outcome,
    );
    assert_eq!(outcome.session_id.as_deref(), Some("th-1"));
    assert_eq!(outcome.usage_output, 4);
    assert_eq!(outcome.usage_cached, 1);
    assert!(outcome.error.is_none());
    feed_stream_line(
        Harness::Codex,
        r#"{"type":"turn.failed","error":{"message":"model not supported"}}"#,
        &mut outcome,
    );
    assert_eq!(outcome.error.as_deref(), Some("model not supported"));
    assert_eq!(outcome.final_text.as_deref(), Some("all set"));
}

#[test]
fn malformed_lines_are_ignored_not_fatal() {
    let mut outcome = RunOutcome::default();
    let note = feed_stream_line(Harness::Claude, "not json at all", &mut outcome);
    assert!(note.is_none());
    assert!(outcome.session_id.is_none());
}

/// Lines captured from real `codex exec --json` (0.154) and
/// `claude -p --output-format stream-json` (2.1.280) runs, trimmed to the
/// fields we read. A hand-written fake once agreed with a wrong parser;
/// these keep the parser honest against the CLIs' actual shapes.
#[test]
fn parses_captured_real_cli_streams() {
    let mut codex = RunOutcome::default();
    for line in [
        r#"{"type":"thread.started","thread_id":"01a0ceb4-8341-7a52-98b6-e8a84e9db9cf"}"#,
        r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"`[features].collab` is deprecated."}}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"hi"}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":19775,"cached_input_tokens":12928,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}"#,
    ] {
        feed_stream_line(Harness::Codex, line, &mut codex);
    }
    assert_eq!(
        codex.session_id.as_deref(),
        Some("01a0ceb4-8341-7a52-98b6-e8a84e9db9cf")
    );
    assert_eq!(codex.final_text.as_deref(), Some("hi"));
    assert_eq!(
        (codex.usage_input, codex.usage_cached, codex.usage_output),
        (19775, 12928, 5)
    );
    assert!(
        codex.error.is_none(),
        "a deprecation item is not a run failure"
    );

    let mut claude = RunOutcome::default();
    for line in [
        r#"{"type":"system","subtype":"init","session_id":"6e8e54b3-2bcb-48aa-9b85-5f9a848efe52","model":"claude-haiku-4-5-20251001"}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"hi","session_id":"6e8e54b3-2bcb-48aa-9b85-5f9a848efe52","total_cost_usd":0.0133868,"usage":{"input_tokens":9,"cache_read_input_tokens":18198,"output_tokens":50}}"#,
    ] {
        feed_stream_line(Harness::Claude, line, &mut claude);
    }
    assert_eq!(
        claude.session_id.as_deref(),
        Some("6e8e54b3-2bcb-48aa-9b85-5f9a848efe52")
    );
    assert_eq!(claude.final_text.as_deref(), Some("hi"));
    assert_eq!(claude.cost_usd, Some(0.0133868));
    assert_eq!(
        (claude.usage_input, claude.usage_cached, claude.usage_output),
        (9, 18198, 50)
    );
    assert!(claude.error.is_none());
}

#[test]
fn a_result_lists_the_calls_the_run_was_refused() {
    let mut outcome = RunOutcome::default();
    feed_stream_line(
        Harness::Claude,
        r#"{"type":"result","result":"x","permission_denials":[{"tool_name":"WebFetch","tool_use_id":"t1","tool_input":{"url":"https://example.com"}},{"tool_use_id":"t2"}]}"#,
        &mut outcome,
    );
    assert_eq!(
        outcome.permission_denials,
        vec![PermissionDenial {
            tool: "WebFetch".to_string(),
            input: serde_json::json!({"url": "https://example.com"}),
        }]
    );
}
