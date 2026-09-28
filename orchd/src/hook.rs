//! The Stop hook's decision function (spec "Stop hook (`hook.stop`)") and
//! the rtk hook's pure parts (`variant.lean_output`): what text a hook
//! payload contributes and each hook's output. Kept as pure functions over
//! facts so they can be unit tested without a socket, a git repo, a
//! classifier call or a real `rtk` binary; `main.rs`/`engine.rs` do the IO
//! and feed them in.

use crate::harness::DENIED_BASH_COMMANDS;
use crate::model::VerifyOutcome;

/// jev-belay-style probabilities the classifier returns for the
/// no-verify-commands path (spec step 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClassifiedOutcome {
    Complete,
    Partial,
    Blocked,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct JevBelayAnswers {
    pub claims_done: f64,
    pub claims_verified: f64,
    pub verification_applies: f64,
    pub outcome: ClassifiedOutcome,
}

pub struct StopFacts<'a> {
    pub blocks_so_far: u32,
    pub has_changed_files: bool,
    pub verify_configured: bool,
    pub verify_results: &'a [VerifyOutcome],
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopDecision {
    Allow,
    Block { reason: String },
}

const MAX_BLOCKS: u32 = 3;
const TAIL_CHARS: usize = 2000;

/// The *last* `max` chars -- the failure detail is usually at the end of a
/// verify command's output, not the start.
fn tail_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

/// Pure decision for `hook.stop`, matching spec steps 1-5 in order.
pub fn decide_stop(facts: &StopFacts, classifier: Option<&JevBelayAnswers>) -> StopDecision {
    if facts.blocks_so_far >= MAX_BLOCKS {
        return StopDecision::Allow;
    }
    if !facts.has_changed_files {
        return StopDecision::Allow;
    }
    if facts.verify_configured {
        if let Some(failed) = facts.verify_results.iter().find(|v| v.code != Some(0)) {
            let code = failed
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string());
            let tail = tail_chars(&failed.tail, TAIL_CHARS);
            return StopDecision::Block {
                reason: format!(
                    "Verification failed: {} exited {}.\n{}\nFix it, run the command again, then finish.",
                    failed.command, code, tail
                ),
            };
        }
        return StopDecision::Allow;
    }

    if let Some(a) = classifier {
        let should_block = a.claims_verified < 0.5
            && a.claims_done >= 0.7
            && a.verification_applies >= 0.5
            && a.outcome != ClassifiedOutcome::Blocked;
        if should_block {
            return StopDecision::Block {
                reason: "You said the work is done, but nothing verified it. Run the relevant checks, then finish.".to_string(),
            };
        }
    }

    StopDecision::Allow
}

// -- rtk hook (`variant.lean_output`) ------------------------------------

/// A PreToolUse payload's `tool_input.command`, the command `orchd hook rtk`
/// asks `rtk rewrite` about. `None` for anything else the hook must fail
/// open on: stdin that isn't JSON, a payload with no `tool_input.command`,
/// or a `command` that isn't a string.
pub fn rtk_hook_command(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("tool_input")?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

/// Whether `command` touches a prefix `orchd hook rtk` must never let past
/// as a rewrite -- checked against both the original and any candidate
/// rewrite, since a rewrite that only introduces the denied prefix (e.g.
/// `rtk git commit ...`) is exactly what would otherwise launder past
/// `Bash(git commit:*)`/`Bash(git push:*)` through the hook's own
/// `permissionDecision: allow`.
pub fn touches_denied_bash_command(command: &str) -> bool {
    DENIED_BASH_COMMANDS.iter().any(|p| command.contains(p))
}

/// `orchd hook rtk`'s reply once `rtk rewrite <original_command>` has run
/// (or failed to). `Some(hookSpecificOutput)` only when it exited 0 with a
/// different, non-empty command that touches no denied git prefix either
/// way; `None` (caller prints `{}`) for everything else -- a non-zero exit
/// (rtk's common "nothing to rewrite" answer), a timeout, rtk missing from
/// PATH, the same command echoed back, or a denied rewrite.
/// rtk subcommands that only condense a build/test tool's output and keep
/// its exit code. `rtk read`/`grep`/`ls` reshape content itself (`rtk read`
/// drops comment lines, and `tail -n` becomes a different `-n`), so a
/// rewrite into them would show the agent code that is not on disk.
const RTK_CONDENSING: [&str; 6] = ["cargo", "npm", "pnpm", "tsc", "pytest", "go"];

fn only_condensing_rtk(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words
        .windows(2)
        .all(|w| w[0] != "rtk" || RTK_CONDENSING.contains(&w[1]))
        && words.last() != Some(&"rtk")
}

/// `rtk rewrite`'s exit code: 0 rewrites and allows, 3 rewrites but leaves
/// the permission decision to Claude Code's own rules (rtk >= 0.50 checks
/// them), 1 has no rewrite and 2 is a deny rule; anything else declines.
pub fn rtk_rewrite_output(
    tool_input: &serde_json::Value,
    original_command: &str,
    rtk_exit: Option<i32>,
    rtk_stdout: &str,
) -> Option<serde_json::Value> {
    let allow = match rtk_exit {
        Some(0) => true,
        Some(3) => false,
        _ => return None,
    };
    let rewritten = rtk_stdout.trim();
    if rewritten.is_empty() || rewritten == original_command.trim() {
        return None;
    }
    if touches_denied_bash_command(original_command) || touches_denied_bash_command(rewritten) {
        return None;
    }
    // `2>&1` only merges streams; any other `>` writes rtk's text to a file.
    let without_merges = original_command.replace("2>&1", "").replace(">&2", "");
    if without_merges.contains('>') || !only_condensing_rtk(rewritten) {
        return None;
    }
    let mut updated_input = tool_input.clone();
    updated_input["command"] = serde_json::Value::String(rewritten.to_string());
    let mut out = serde_json::json!({
        "hookEventName": "PreToolUse",
        "updatedInput": updated_input,
    });
    if allow {
        out["permissionDecision"] = "allow".into();
    }
    Some(serde_json::json!({ "hookSpecificOutput": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(
        blocks: u32,
        changed: bool,
        verify_configured: bool,
        verify: &'a [VerifyOutcome],
    ) -> StopFacts<'a> {
        StopFacts {
            blocks_so_far: blocks,
            has_changed_files: changed,
            verify_configured,
            verify_results: verify,
        }
    }

    #[test]
    fn allows_once_three_blocks_reached_for_this_attempt() {
        let d = decide_stop(&facts(3, true, true, &[]), None);
        assert_eq!(d, StopDecision::Allow);
        let d = decide_stop(&facts(4, true, true, &[]), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn allows_when_no_changed_files() {
        let d = decide_stop(&facts(0, false, true, &[]), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn blocks_with_failing_verify_tail() {
        let verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "FAIL src/x.test.ts".into(),
            ms: 100,
        }];
        let d = decide_stop(&facts(0, true, true, &verify), None);
        match d {
            StopDecision::Block { reason } => {
                assert!(reason.contains("npm test exited 1"));
                assert!(reason.contains("FAIL src/x.test.ts"));
            }
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn allows_when_verify_passes() {
        let verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(0),
            tail: "ok".into(),
            ms: 100,
        }];
        let d = decide_stop(&facts(0, true, true, &verify), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn verify_tail_keeps_the_last_2000_chars_not_the_first() {
        // The interesting part of a failing command's output (the actual
        // assertion/error) is almost always at the end, not the start.
        let long_tail = format!("{}END_MARKER", "x".repeat(5000));
        let verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: long_tail,
            ms: 100,
        }];
        let d = decide_stop(&facts(0, true, true, &verify), None);
        match d {
            StopDecision::Block { reason } => {
                assert!(reason.len() < 5100);
                assert!(
                    reason.contains("END_MARKER"),
                    "should keep the tail end of the output, not the head: {reason}"
                );
            }
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn no_verify_commands_blocks_on_jev_belay_thresholds() {
        let a = JevBelayAnswers {
            claims_done: 0.9,
            claims_verified: 0.2,
            verification_applies: 0.8,
            outcome: ClassifiedOutcome::Complete,
        };
        let d = decide_stop(&facts(0, true, false, &[]), Some(&a));
        match d {
            StopDecision::Block { reason } => assert!(reason.contains("nothing verified it")),
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn no_verify_commands_allows_when_outcome_is_blocked() {
        let a = JevBelayAnswers {
            claims_done: 0.9,
            claims_verified: 0.2,
            verification_applies: 0.8,
            outcome: ClassifiedOutcome::Blocked,
        };
        let d = decide_stop(&facts(0, true, false, &[]), Some(&a));
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn no_verify_commands_and_no_classifier_allows() {
        let d = decide_stop(&facts(0, true, false, &[]), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn no_verify_commands_allows_below_thresholds() {
        let a = JevBelayAnswers {
            claims_done: 0.5, // below 0.7 threshold
            claims_verified: 0.2,
            verification_applies: 0.8,
            outcome: ClassifiedOutcome::Complete,
        };
        let d = decide_stop(&facts(0, true, false, &[]), Some(&a));
        assert_eq!(d, StopDecision::Allow);
    }

    // -- rtk hook --

    #[test]
    fn rtk_hook_command_reads_tool_input_command_or_fails_open() {
        assert_eq!(
            rtk_hook_command(&serde_json::json!({"tool_input": {"command": "npm test"}})),
            Some("npm test".to_string())
        );
        assert_eq!(rtk_hook_command(&serde_json::Value::Null), None);
        assert_eq!(rtk_hook_command(&serde_json::json!({})), None);
        assert_eq!(
            rtk_hook_command(&serde_json::json!({"tool_input": {}})),
            None
        );
        assert_eq!(
            rtk_hook_command(&serde_json::json!({"tool_input": {"command": 1}})),
            None
        );
    }

    #[test]
    fn rtk_rewrite_output_allows_only_a_clean_different_rewrite() {
        let input =
            serde_json::json!({"command": "npm test", "description": "run tests", "timeout": 1000});
        let out = rtk_rewrite_output(&input, "npm test", Some(0), "rtk npm test\n").unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"],
            serde_json::json!({
                "command": "rtk npm test",
                "description": "run tests",
                "timeout": 1000
            })
        );

        // Same command back, non-zero exit, empty output: all decline.
        assert!(rtk_rewrite_output(&input, "npm test", Some(0), "npm test").is_none());
        assert!(rtk_rewrite_output(&input, "npm test", Some(1), "npm test --silent").is_none());
        assert!(rtk_rewrite_output(&input, "npm test", Some(0), "").is_none());
        assert!(rtk_rewrite_output(&input, "npm test", Some(0), "   ").is_none());
    }

    #[test]
    fn rtk_rewrite_output_leaves_an_ask_answer_to_claude_code() {
        let input = serde_json::json!({"command": "cargo test"});
        let out = rtk_rewrite_output(&input, "cargo test", Some(3), "rtk cargo test").unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk cargo test"
        );
        assert!(out["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
        assert!(rtk_rewrite_output(&input, "cargo test", Some(2), "rtk cargo test").is_none());
        assert!(rtk_rewrite_output(&input, "cargo test", None, "rtk cargo test").is_none());
    }

    #[test]
    fn rtk_rewrite_output_declines_content_changing_rewrites() {
        // Real rtk 0.28 rewrites that alter what the agent sees or writes.
        for (orig, rewritten) in [
            ("cat a.rs > b.rs", "rtk read a.rs > b.rs"),
            ("cat a.rs", "rtk read a.rs"),
            ("tail -n 50 log", "rtk read -n 50 log"),
            ("rg foo", "rtk grep foo"),
            ("ls -la", "rtk ls -la"),
            ("npx eslint .", "rtk lint ."),
            ("cargo test > out.txt", "rtk cargo test > out.txt"),
        ] {
            let input = serde_json::json!({ "command": orig });
            assert!(
                rtk_rewrite_output(&input, orig, Some(0), rewritten).is_none(),
                "{orig}"
            );
        }
        for (orig, rewritten) in [
            ("cargo test", "rtk cargo test"),
            (
                "cargo build && git status | head -5",
                "rtk cargo build && git status | head -5",
            ),
            ("tsc --noEmit", "rtk tsc --noEmit"),
            (
                "cargo test --manifest-path orchd/Cargo.toml 2>&1 | tail -40",
                "rtk cargo test --manifest-path orchd/Cargo.toml 2>&1 | tail -40",
            ),
        ] {
            let input = serde_json::json!({ "command": orig });
            assert!(
                rtk_rewrite_output(&input, orig, Some(0), rewritten).is_some(),
                "{orig}"
            );
        }
    }

    #[test]
    fn rtk_rewrite_output_never_rewrites_past_git_commit_or_push() {
        let input = serde_json::json!({"command": "git commit -m x"});
        assert!(
            rtk_rewrite_output(&input, "git commit -m x", Some(0), "git commit -am x").is_none()
        );
        let input = serde_json::json!({"command": "git push"});
        assert!(rtk_rewrite_output(&input, "git push", Some(0), "git push origin main").is_none());
        // Even an unrelated original must not be rewritten into one.
        let input = serde_json::json!({"command": "ls"});
        assert!(rtk_rewrite_output(&input, "ls", Some(0), "rtk git commit -am x").is_none());
        assert!(touches_denied_bash_command("git commit -m x"));
        assert!(touches_denied_bash_command("git push origin main"));
        assert!(!touches_denied_bash_command("git status"));
    }
}
