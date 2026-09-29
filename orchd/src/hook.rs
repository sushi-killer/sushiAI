//! The Stop hook's decision function (spec "Stop hook (`hook.stop`)"), kept
//! as a pure function over facts so it can be unit tested without a socket,
//! a git repo or a model call; `main.rs`/`engine.rs` do the IO and feed
//! them in.

use crate::model::VerifyOutcome;

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

/// Pure decision for `hook.stop`, matching the spec steps in order.
pub fn decide_stop(facts: &StopFacts) -> StopDecision {
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

    StopDecision::Allow
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
        let d = decide_stop(&facts(3, true, true, &[]));
        assert_eq!(d, StopDecision::Allow);
        let d = decide_stop(&facts(4, true, true, &[]));
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn allows_when_no_changed_files() {
        let d = decide_stop(&facts(0, false, true, &[]));
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
        let d = decide_stop(&facts(0, true, true, &verify));
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
        let d = decide_stop(&facts(0, true, true, &verify));
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
        let d = decide_stop(&facts(0, true, true, &verify));
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
    fn no_verify_commands_allows() {
        let d = decide_stop(&facts(0, true, false, &[]));
        assert_eq!(d, StopDecision::Allow);
    }
}
