use crate::delivery::pr::body::final_body;
use crate::delivery::pr::body::redact_pr_text;
use crate::delivery::pr::failure::classify_external;
use crate::delivery::pr::failure::failure_of;
use crate::delivery::pr::failure::PrFailure;
use crate::delivery::pr::reply::draft_schema;
use crate::delivery::pr::reply::schema_reply;
use crate::delivery::pr::reply::BodyCheck;
use crate::delivery::pr::reply::Draft;
use crate::harness;
use crate::policy::routing::Harness;
use crate::task::questions::Assumption;
use crate::task::questions::QuestionKind;
use crate::task::questions::WAIVED;
use crate::task::questions::WAIVER_BY;
use crate::task::state::TaskStatus;
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::task_with_status;

    #[test]
    fn the_waived_section_survives_a_drafted_and_a_rewritten_body() {
        let mut task = task_with_status(TaskStatus::Done);
        task.record_assumption(Assumption {
            question: "q".into(),
            answer: WAIVED.into(),
            evidence: "alpha fails on the base".into(),
            by: WAIVER_BY.into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: Some(1),
            commands: vec!["check".into()],
            overturned: false,
            owner_answer: None,
        });
        let none = std::collections::HashMap::new();
        for drafted in [
            "Why this change\n",
            "Why\n\n## Waived base failures\n\n- invented\n",
        ] {
            let body = final_body(&task, drafted.to_string(), &none, None, &Default::default());
            assert_eq!(body.matches("## Waived base failures").count(), 1, "{body}");
            assert!(body.contains("- check: alpha fails on the base"), "{body}");
            assert!(!body.contains("invented"), "{body}");
        }
        task.fixture_data().unreviewed = true;
        let body = final_body(&task, "Why\n".into(), &none, None, &Default::default());
        assert!(
            body.starts_with(crate::delivery::pr::body::UNREVIEWED_LINE)
                && body.contains("alpha fails")
        );
    }

    #[test]
    fn a_schema_checked_reply_is_read_and_anything_else_fails_the_turn() {
        let json = r#"{"branch":"feature/x","title":"feat: x","body":"Why\n"}"#;
        let want = Draft {
            branch: "feature/x".into(),
            title: "feat: x".into(),
            body: "Why\n".into(),
        };
        let reply = |text: &str| {
            let stream = format!(
                "{}\n",
                serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "result": text})
            );
            harness::replay_events(Harness::Claude, &stream)
        };
        assert_eq!(schema_reply(&reply(json), "draft"), Ok(want));
        for bad in [
            format!("```json\n{json}\n```"),
            format!("Here you go: {json} done"),
            r#"{"title":"t","body":""}"#.to_string(),
            r#"{"branch":"b","title":5,"body":""}"#.to_string(),
            "no json here".to_string(),
        ] {
            assert_eq!(
                schema_reply::<Draft>(&reply(&bad), "draft"),
                Err("the reply did not match the draft schema".to_string()),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_body_check_needs_an_explicit_verdict() {
        let fixed: BodyCheck = serde_json::from_str(
            r#"{"ok":false,"corrections":["the diff adds no tests"],"body":"Fixed"}"#,
        )
        .unwrap();
        assert!(!fixed.ok);
        assert_eq!(
            fixed.corrections,
            vec!["the diff adds no tests".to_string()]
        );
        assert_eq!(fixed.body.as_deref(), Some("Fixed"));
        let clean: BodyCheck =
            serde_json::from_str(r#"{"ok":true,"corrections":[],"body":null}"#).unwrap();
        assert!(clean.ok && clean.body.is_none());
        assert!(serde_json::from_str::<BodyCheck>(r#"{"corrections":[],"body":null}"#).is_err());
    }

    #[test]
    fn the_draft_turn_passes_its_schema_to_each_harness() {
        let wt = Path::new("/worktree");
        let schema_path = Path::new("/run/schema.json");
        let schema = harness::OutputSchema {
            json: draft_schema(),
            path: schema_path,
        };
        let req = |harness| harness::RunRequest {
            harness,
            worktree: wt,
            model: None,
            effort: None,
            max_budget_usd: None,
            review: true,
            task_run: false,
            task_sandbox: None,
            mcp_config: None,
            settings_path: None,
            network_allowed: false,
            codex_mcp: &[],
            images: &[],
            repo_settings: true,
            resume: None,
        };
        let claude = harness::build_argv_with_schema(&req(Harness::Claude), Some(&schema));
        let flag = claude.iter().position(|a| a == "--json-schema").unwrap();
        assert_eq!(claude[flag + 1], draft_schema());
        let codex = harness::build_argv_with_schema(&req(Harness::Codex), Some(&schema));
        let flag = codex.iter().position(|a| a == "--output-schema").unwrap();
        assert_eq!(codex[flag + 1], "/run/schema.json");
        assert_eq!(codex.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn redaction_names_secrets_and_hides_the_home_directory() {
        let secrets = std::collections::HashMap::from([
            ("API_KEY".to_string(), "s3cr3t-value".to_string()),
            ("SHORT".to_string(), "s3cr3t".to_string()),
            ("EMPTY".to_string(), String::new()),
        ]);
        let out = redact_pr_text(
            "key s3cr3t-value and s3cr3t at /home/zed/proj/a.rs",
            &secrets,
            Some("/home/zed/"),
        );
        assert_eq!(out, "key ${API_KEY} and ${SHORT} at ~/proj/a.rs");
        assert_eq!(redact_pr_text("/x", &secrets, Some("/")), "/x");
        assert_eq!(redact_pr_text("plain", &secrets, None), "plain");
    }

    #[test]
    fn a_rate_limit_or_a_timeout_is_transient_even_with_a_403() {
        for e in [
            "gh pr: HTTP 403: API rate limit exceeded",
            "gh pr failed: HTTP 403: You have exceeded a secondary rate limit",
            "git ls-remote timed out after 60s",
            "gh pr failed: HTTP 502: Bad Gateway, try again later",
            "git push failed: Could not resolve host: example.invalid",
            "gh pr failed: connection reset by peer",
        ] {
            assert!(matches!(classify_external(e), PrFailure::Transient), "{e}");
        }
        for e in [
            "gh pr failed: To get started, please run:  gh auth login",
            "git push failed: Permission denied (publickey)",
            "git push failed: remote: error: GH006: Protected branch update failed",
            "gh pr failed: HTTP 401: Bad credentials",
            "git fetch failed: fatal: 'origin' does not appear to be a git repository",
        ] {
            assert!(
                matches!(classify_external(e), PrFailure::Permanent(_)),
                "{e}"
            );
        }
        let scope = "remote: refusing to allow a Personal Access Token to create or update workflow `.github/workflows/ci.yml` without `workflow` scope";
        let PrFailure::Permanent(hint) = classify_external(scope) else {
            panic!("a scope refusal is permanent");
        };
        assert!(hint.contains("workflow"), "{hint}");
        assert!(matches!(
            classify_external("none of the git remotes point to a known GitHub host"),
            PrFailure::NoGithubHost
        ));
    }

    #[test]
    fn an_orchd_refusal_is_decided_by_its_variant_not_its_text() {
        let carry = crate::delivery::pr::error::PrError::Refused(
            crate::delivery::pr::error::PrRefusal::CarryFailed(
                "could not reach origin; try again later".into(),
            ),
        );
        assert!(matches!(failure_of(&carry), PrFailure::Permanent(_)));
        let busy = crate::delivery::pr::error::PrError::Refused(
            crate::delivery::pr::error::PrRefusal::InFlight,
        );
        assert!(matches!(failure_of(&busy), PrFailure::Transient));
        let push = crate::delivery::pr::error::PrError::External(
            "git push failed: connection reset".into(),
        );
        assert!(matches!(failure_of(&push), PrFailure::Transient));
    }
}
