//! The Stop hook's decision function (spec "Stop hook (`hook.stop`)").
//! Kept as a pure function over facts so it can be unit tested without a
//! socket, a git repo, or a classifier call; `protocol.rs`/`engine.rs` do
//! the IO and feed it in.

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
}
