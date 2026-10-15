use crate::harness::{feed_stream_line, replay_events, RunOutcome};
use crate::policy::routing::Harness;

/// Shape captured from `claude -p --output-format stream-json --verbose`
/// (2.1.283): the first turn read most of its prompt from the cache.
#[test]
fn first_turn_tokens_count_cached_prompt_tokens_from_the_first_assistant_event() {
    let mut outcome = RunOutcome::default();
    for line in [
        r#"{"type":"system","subtype":"init","session_id":"s"}"#,
        r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":9,"cache_creation_input_tokens":11644,"cache_read_input_tokens":18198,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":11644},"output_tokens":4,"service_tier":"standard"}},"session_id":"s"}"#,
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":5,"cache_read_input_tokens":40000}},"session_id":"s"}"#,
        r#"{"type":"result","usage":{"input_tokens":14,"cache_read_input_tokens":58198,"output_tokens":9},"result":"ok"}"#,
    ] {
        feed_stream_line(Harness::Claude, line, &mut outcome);
    }
    assert_eq!(outcome.first_turn_tokens, Some(9 + 11644 + 18198));

    let mut none = RunOutcome::default();
    feed_stream_line(
        Harness::Claude,
        r#"{"type":"result","usage":{"input_tokens":1},"result":"ok"}"#,
        &mut none,
    );
    assert_eq!(none.first_turn_tokens, None);
}

/// Shape of `claude -p --output-format stream-json --verbose` (2.1.283)
/// cut off before its `result`: one message streamed as two events (a
/// text block, then a tool_use block) under the same id, a second
/// message, a subagent's message, and a stderr line from events.jsonl.
#[test]
fn a_run_without_a_result_is_priced_from_its_unique_messages_and_marked_estimated() {
    let events = [
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5-5"}"#,
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_01A","type":"message","role":"assistant","content":[{"type":"text","text":"Reading."}],"usage":{"input_tokens":6,"cache_creation_input_tokens":12000,"cache_read_input_tokens":18000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":12000},"output_tokens":30,"service_tier":"standard"}},"parent_tool_use_id":null,"session_id":"s"}"#,
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_01A","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/w/a.rs"}}],"usage":{"input_tokens":6,"cache_creation_input_tokens":12000,"cache_read_input_tokens":18000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":12000},"output_tokens":30,"service_tier":"standard"}},"parent_tool_use_id":null,"session_id":"s"}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"fn a() {}"}]},"parent_tool_use_id":null,"session_id":"s"}"#,
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_01B","type":"message","role":"assistant","content":[{"type":"text","text":"Editing."}],"usage":{"input_tokens":2,"cache_creation_input_tokens":500,"cache_read_input_tokens":30000,"output_tokens":400,"service_tier":"standard"}},"parent_tool_use_id":null,"session_id":"s"}"#,
        r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_sub","type":"message","role":"assistant","content":[],"usage":{"input_tokens":9999,"output_tokens":9999}},"parent_tool_use_id":"toolu_9","session_id":"s"}"#,
        "[stderr] warning: something",
    ]
    .join("\n");
    let mut outcome = replay_events(Harness::Claude, &events);
    assert_eq!(outcome.messages.len(), 2);
    assert_eq!(outcome.cost_usd, None);
    let prices = crate::policy::settings::Settings::default().prices;
    outcome.estimate_cost(&prices);
    let expected = (6.0 * 4.0 + 12_000.0 * 5.0 + 18_000.0 * 0.20 + 30.0 * 20.0) / 1e6
        + (2.0 * 4.0 + 500.0 * 5.0 + 30_000.0 * 0.20 + 400.0 * 20.0) / 1e6;
    let cost = outcome.cost_usd.unwrap();
    assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");
    assert!(outcome.cost_estimated);
    assert_eq!(
        (
            outcome.usage_input,
            outcome.usage_cached,
            outcome.usage_output
        ),
        (8, 48_000, 430)
    );

    // A run that did print its result keeps the CLI's own figure.
    let finished = format!(
        "{events}\n{}",
        r#"{"type":"result","total_cost_usd":0.5,"usage":{"input_tokens":8},"result":"ok"}"#
    );
    let mut outcome = replay_events(Harness::Claude, &finished);
    outcome.estimate_cost(&prices);
    assert_eq!(outcome.cost_usd, Some(0.5));
    assert!(!outcome.cost_estimated);

    // No price for the model: no guess.
    let mut unpriced = replay_events(Harness::Claude, &events.replace("claude-opus-5-5", "x"));
    unpriced.estimate_cost(&prices);
    assert_eq!(unpriced.cost_usd, None);
    assert!(!unpriced.cost_estimated);
}
