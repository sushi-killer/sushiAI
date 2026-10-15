use super::native::head_chars;
use super::run::{ReviewMode, ReviewSession};
use super::types::ReviewFailure;
use crate::engine::test_support::slow_helper_task;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::helper_timeout_message;
use std::path::Path;
#[cfg(test)]
mod cases {
    use super::head_chars;

    #[test]
    fn a_long_reply_is_quoted_from_its_start() {
        let reply = format!("START{}END", "x".repeat(600));
        let quote = head_chars(&reply, 300);
        assert!(quote.starts_with("STARTxxx"));
        assert!(!quote.contains("END"));
        assert!(quote.ends_with('\u{2026}'));
        assert_eq!(quote.chars().count(), 301);
        assert_eq!(head_chars("short", 300), "short");
    }
}

#[cfg(test)]
mod is_test_path_tests {
    use crate::policy::paths::is_test_path;

    #[test]
    fn test_directories_and_file_names_count() {
        for path in [
            "test/a.rs",
            "src/tests/a.rs",
            "web/__tests__/a.ts",
            "spec/a.rb",
            "src/specs/a.rb",
            "e2e/login.ts",
            "src/panel.test.ts",
            "src/panel.spec.ts",
            "src/panel_test.go",
            "tools/test_panel.py",
        ] {
            assert!(is_test_path(path), "{path}");
        }
    }

    #[test]
    fn near_misses_do_not_count() {
        for path in [
            "src/contest.rs",
            "testing/x.rs",
            "src/latest.rs",
            "src/attest_all.rs",
            "src/testimony.rs",
            "specification/a.md",
            "",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn reviewer_timed_out_takes_the_harness_failure_path() {
        let (app, _dir, mut task, route) = slow_helper_task();
        let cancel = CancelToken::new();
        let session = ReviewSession::new(
            &task,
            1,
            &route,
            &cancel,
            ReviewMode {
                after_no_verdict: false,
                force_full: false,
            },
        );
        let report = crate::engine::lifecycle::report::ImplementerReport {
            final_text: String::new(),
            report: None,
            note: String::new(),
            decisions: Vec::new(),
        };
        let result = session.run(&app, &mut task, &report, &[]).await.result;
        assert!(
            matches!(result, Err(ReviewFailure::Harness(ref e)) if e.contains("reviewer timed out after 0.25s"))
        );
        // The route turn runs first on the same slow fake: two calls, and the
        // timed-out reviewer is still not started again.
        let line = helper_timeout_message("reviewer", std::time::Duration::from_millis(250));
        assert_eq!(
            task.decisions.iter().filter(|d| d.contains(&line)).count(),
            1,
            "{:?}",
            task.decisions
        );
        assert!(
            crate::engine::chat::test_support::fake_overseer_calls(Path::new(&task.worktree)) <= 2
        );
    }
}

#[test]
fn a_timed_out_review_retains_spend_and_fingerprint_even_with_a_pass_in_its_output() {
    let fingerprint = crate::task::Fingerprint {
        models: vec!["review-model".into()],
        harness: crate::policy::routing::Harness::Codex,
        harness_version: Some("test-version".into()),
        prompt_hash: "review-prompt".into(),
    };
    let receipt = super::execution::ReviewExecution::completed(
        crate::harness::RunOutcome {
            final_text: Some(r#"{"verdict":"pass","findings":[]}"#.into()),
            error: Some("reviewer timed out after 2s".into()),
            timed_out: true,
            cost_usd: Some(0.125),
            fingerprint: Some(fingerprint.clone()),
            ..Default::default()
        },
        std::time::Duration::from_secs(2),
    );
    assert!(
        matches!(receipt.outcome, Err(ReviewFailure::Harness(ref why)) if why.contains("timed out"))
    );
    assert_eq!(receipt.cost_usd, 0.125);
    assert_eq!(receipt.fingerprint, Some(fingerprint));
    assert_eq!(
        receipt.timeout_note.as_deref(),
        Some("reviewer timed out after 2s")
    );
}
