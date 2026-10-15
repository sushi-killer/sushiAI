//! The live detector consumes both harness stream formats. Red results are
//! evidence for the harness; exact repeated actions remain bounded by orchd.

use sushiai_orch::loop_detect::LoopDetector;

#[test]
fn different_commands_with_identical_red_results_can_finish_their_stream() {
    for stream in [
        include_str!("fixtures/loop_detect/claude-identical-red-results.jsonl"),
        include_str!("fixtures/loop_detect/codex-identical-red-results.jsonl"),
    ] {
        let mut detector = LoopDetector::new();
        for (line_n, line) in stream.lines().enumerate() {
            assert_eq!(detector.feed(line), None, "line {}: {line}", line_n + 1);
        }
    }
}

#[test]
fn identical_commands_still_stop_at_the_third_call() {
    for stream in [
        include_str!("fixtures/loop_detect/claude-repeated-call.jsonl"),
        include_str!("fixtures/loop_detect/codex-repeated-call.jsonl"),
    ] {
        let mut detector = LoopDetector::new();
        let (line_n, hit) = stream
            .lines()
            .enumerate()
            .find_map(|(n, line)| detector.feed(line).map(|hit| (n + 1, hit)))
            .expect("the repeated call must stop the stream");
        assert_eq!(line_n, 5);
        assert_eq!(hit.rule, "repeated_call");
        assert!(hit.detail.contains("cargo test check_1"));
    }
}

#[test]
fn changing_edits_to_one_file_still_stop_at_the_eighth_edit() {
    for (stream, expected_line) in [
        (
            include_str!("fixtures/loop_detect/claude-repeated-edit.jsonl"),
            15,
        ),
        (
            include_str!("fixtures/loop_detect/codex-repeated-edit.jsonl"),
            8,
        ),
    ] {
        let mut detector = LoopDetector::new();
        let (line_n, hit) = stream
            .lines()
            .enumerate()
            .find_map(|(n, line)| detector.feed(line).map(|hit| (n + 1, hit)))
            .expect("the repeated edit must stop the stream");
        assert_eq!(line_n, expected_line);
        assert_eq!(hit.rule, "repeated_edit");
        assert!(hit.detail.contains("src/check.rs"));
    }
}
