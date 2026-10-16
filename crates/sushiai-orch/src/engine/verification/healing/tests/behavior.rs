use crate::engine::test_support::attempt_with_failure;
use crate::engine::test_support::task_with_status;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::commands::fix_grep_dashes;
use crate::task::checks::edits::heal_grep_dashes;
use crate::task::review::CriterionRuling;
use crate::task::review::FindingDetail;
use crate::task::review::ReviewResult;
use crate::task::review::Verdict;
use crate::task::state::TaskStatus;
use crate::verify::healing::reply::new_answers;
use crate::verify::healing::reply::parse_amendments;
use crate::verify::healing::reply::parse_feasibility;
use crate::verify::healing::reply::parse_ruling;
use crate::verify::healing::reply::repeated_unmet;
use crate::verify::healing::reply::Feasibility;
use crate::verify::healing::reply::Ruling;
use crate::verify::healing::scope::own_changes_since;
use crate::verify::healing::scope::same_failure;
use crate::verify::healing::scope::scope_review;

#[cfg(test)]
mod cases {
    use super::*;

    fn failed_run(failing: &[&str]) -> VerifyOutcome {
        VerifyOutcome {
            command: "cargo test".into(),
            code: Some(1),
            output_key: "key".into(),
            failing: failing.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_finding_is_old_only_when_the_same_named_tests_fail_on_both_trees() {
        let base = failed_run(&["a::bad"]);
        assert!(same_failure(&failed_run(&["a::bad"]), &base));
        // The task tree fails other tests, or the runs name none (`false`).
        assert!(!same_failure(&failed_run(&["a::bad", "b::new"]), &base));
        assert!(!same_failure(&failed_run(&["b::other"]), &base));
        assert!(!same_failure(&failed_run(&[]), &failed_run(&[])));
        assert!(!same_failure(&failed_run(&[]), &base));
    }

    #[test]
    fn grep_patterns_starting_with_a_dash_get_a_double_dash() {
        assert_eq!(
            fix_grep_dashes("grep -qF '- `formatDuration` x' README.md").unwrap(),
            "grep -qF -- '- `formatDuration` x' README.md"
        );
        assert_eq!(
            fix_grep_dashes("test -f a && grep -q \"-x\" a | grep -c -v '-y' b").unwrap(),
            "test -f a && grep -q -- \"-x\" a | grep -c -v -- '-y' b"
        );
        for fine in [
            "grep -qF -- '- x' README.md",
            "grep -qF 'x' README.md",
            "grep -e '-x' a",
            "grep -qe '-x' a",
            "echo grep '-x'",
            "grep -rn foo src",
            "grep -rn --include='*.ts' foo src",
            "grep --regexp='-x' src",
            "grep --include='*.ts' --regexp=foo a",
        ] {
            assert_eq!(fix_grep_dashes(fine), None, "{fine}");
        }
    }

    #[test]
    fn a_pattern_after_quoted_long_options_still_gets_the_double_dash() {
        assert_eq!(
            fix_grep_dashes("grep -rn --include='*.ts' '- x' src").unwrap(),
            "grep -rn --include='*.ts' -- '- x' src"
        );
    }

    #[test]
    fn a_broken_grep_check_is_fixed_everywhere_with_one_decision_line() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        task.fixture_data().verify = vec!["grep -qF '- x' R.md".into(), "npm test".into()];
        task.fixture_data().final_verify = vec!["grep -qF '- x' R.md".into()];
        task.fixture_data().decisions.clear();
        assert!(heal_grep_dashes(&mut task));
        assert_eq!(task.verify, ["grep -qF -- '- x' R.md", "npm test"]);
        assert_eq!(task.final_verify, ["grep -qF -- '- x' R.md"]);
        assert_eq!(task.decisions.len(), 1);
        assert!(!heal_grep_dashes(&mut task));
    }

    fn scoped(findings: &[(&str, Option<bool>)], criteria: Vec<CriterionRuling>) -> ReviewResult {
        let mut findings_text: Vec<String> =
            findings.iter().map(|(text, _)| text.to_string()).collect();
        findings_text.extend(
            criteria
                .iter()
                .filter(|r| r.met != Some(true))
                .map(|r| format!("criterion finding: {}", r.criterion)),
        );
        ReviewResult {
            verdict: Verdict::Fail,
            findings: findings_text,
            repeated: Vec::new(),
            severities: findings
                .iter()
                .map(|(t, _)| crate::task::review::severity_of(t))
                .collect(),
            details: findings
                .iter()
                .map(|(_, caused)| FindingDetail {
                    caused_by_diff: *caused,
                    ..Default::default()
                })
                .collect(),
            criteria,
        }
    }

    fn ruling(index: usize, met: Option<bool>) -> CriterionRuling {
        CriterionRuling {
            criterion: format!("c{index}"),
            index: Some(index),
            met,
            evidence: None,
        }
    }

    #[test]
    fn only_a_p1_shown_old_on_the_base_is_demoted_and_the_rest_block() {
        let mut review = scoped(
            &[
                (
                    "P1: callers/untouched.rs:9 - breaks after the change",
                    Some(true),
                ),
                ("P1: pre-existing.rs:2 - was already broken", Some(false)),
                ("P1: claimed old, not shown", Some(false)),
                ("P1: no mark at all", None),
            ],
            Vec::new(),
        );
        let follow_ups = scope_review(&mut review, &[1]);
        assert_eq!(
            review.findings,
            [
                "P1: callers/untouched.rs:9 - breaks after the change",
                "P1: claimed old, not shown",
                "P1: no mark at all"
            ]
        );
        assert_eq!(follow_ups, ["P1: pre-existing.rs:2 - was already broken"]);
        assert_eq!(review.verdict, Verdict::Fail);
    }

    #[test]
    fn a_p0_and_an_unmet_criterion_always_block_and_p2_never_does() {
        let mut review = scoped(
            &[("P0: crash", Some(false)), ("P2: smell", Some(true))],
            vec![
                ruling(0, Some(false)),
                ruling(1, None),
                ruling(2, Some(true)),
            ],
        );
        let follow_ups = scope_review(&mut review, &[]);
        assert_eq!(review.findings, ["P0: crash", "criterion finding: c0"]);
        assert_eq!(follow_ups, ["P2: smell", "criterion finding: c1"]);
        assert_eq!(review.verdict, Verdict::Fail);
        let mut only_pre_existing = scoped(&[("P1: old", Some(false))], Vec::new());
        scope_review(&mut only_pre_existing, &[0]);
        assert_eq!(only_pre_existing.verdict, Verdict::Pass);
    }

    #[test]
    fn a_finding_that_came_back_blocks_whoever_caused_it() {
        let mut review = scoped(&[("P1: back [F-1]", Some(false))], Vec::new());
        review.repeated = vec!["P1: back [F-1]".into()];
        assert!(scope_review(&mut review, &[0]).is_empty());
        assert_eq!(review.findings.len(), 1);
    }

    #[test]
    fn two_criteria_where_one_text_prefixes_the_other_stay_distinct() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.fixture_data().criteria = vec!["Beta".into(), "Beta works -- check: read it".into()];
        let review = |met: [Option<bool>; 2]| ReviewResult {
            verdict: Verdict::Fail,
            findings: Vec::new(),
            repeated: Vec::new(),
            severities: Vec::new(),
            details: Vec::new(),
            criteria: (0..2).map(|i| ruling(i, met[i])).collect(),
        };
        let mut first = attempt_with_failure(1, "x");
        first.review = Some(review([Some(false), Some(true)]));
        task.fixture_data().attempts = vec![first];
        let now = review([Some(false), Some(true)]);
        let rep = repeated_unmet(&task, 1, 2, &now).expect("criterion 0 repeats");
        assert_eq!(rep.criterion, 0);
        let only_longer = review([Some(true), Some(false)]);
        assert!(repeated_unmet(&task, 1, 2, &only_longer).is_none());
    }

    #[test]
    fn feasibility_replies_are_read_leniently() {
        assert!(matches!(
            parse_feasibility("{\"verdict\":\"feasible\"}", 1),
            Some(Feasibility::AsWritten)
        ));
        assert!(matches!(
            parse_feasibility("x {\"verdict\":\"rewrite\",\"command\":\"true\"} y", 1),
            Some(Feasibility::Rewrite { .. })
        ));
        assert!(parse_feasibility("{\"verdict\":\"rewrite\"}", 1).is_none());
        assert!(parse_feasibility("nothing", 1).is_none());
    }

    #[test]
    fn a_ruling_names_infeasible_or_code_gap() {
        assert!(matches!(
            parse_ruling("{\"verdict\":\"infeasible as written\",\"criterion\":\"c\"}"),
            Some(Ruling::Infeasible { .. })
        ));
        assert!(matches!(
            parse_ruling("{\"verdict\":\"code gap\"}"),
            Some(Ruling::CodeGap)
        ));
        assert!(parse_ruling("{\"verdict\":\"infeasible as written\"}").is_none());
    }

    #[test]
    fn amendments_outside_the_criteria_are_dropped() {
        let text = "{\"amend\":[{\"criterion\":0,\"text\":\"a\"},{\"criterion\":5,\"text\":\"b\"},{\"criterion\":1,\"text\":\"\"}]}";
        assert_eq!(parse_amendments(text, 2), vec![(0, "a".to_string())]);
    }

    fn evidence_repo() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        impl Fn(&[&str]) -> String,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let dir = repo.clone();
        let git = move |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "hi\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        (tmp, repo, git)
    }

    #[test]
    fn evidence_survives_carried_in_base_commits_but_not_the_tasks_own_edit() {
        let (_tmp, repo, git) = evidence_repo();
        let base1 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "task"]);
        std::fs::write(repo.join("Button.tsx"), "v1\n").unwrap();
        let old_tree = crate::git::changes::worktree_tree(&repo).unwrap();
        git(&["checkout", "-q", "-b", "other", &base1]);
        std::fs::create_dir_all(repo.join("orchd")).unwrap();
        std::fs::write(repo.join("Landed.tsx"), "l\n").unwrap();
        std::fs::write(repo.join("orchd/x.rs"), "r\n").unwrap();
        git(&["add", "Landed.tsx", "orchd"]);
        git(&["commit", "-q", "-m", "landed"]);
        let base2 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "task"]);
        std::fs::write(repo.join("Landed.tsx"), "l\n").unwrap();
        std::fs::create_dir_all(repo.join("orchd")).unwrap();
        std::fs::write(repo.join("orchd/x.rs"), "r\n").unwrap();
        // Only carried-in commits changed files: reuse.
        assert!(own_changes_since(&repo, &old_tree, Some(&base1), &base2)
            .unwrap()
            .is_empty());
        // Without the earlier base the carried-in file looks like a change.
        assert_eq!(
            own_changes_since(&repo, &old_tree, None, &base2).unwrap(),
            vec!["Landed.tsx".to_string(), "orchd/x.rs".to_string()]
        );
        // The task edits its own file: new evidence needed.
        std::fs::write(repo.join("Button.tsx"), "v2\n").unwrap();
        assert_eq!(
            own_changes_since(&repo, &old_tree, Some(&base1), &base2).unwrap(),
            vec!["Button.tsx".to_string()]
        );
    }

    #[test]
    fn a_conflict_resolution_in_any_non_test_file_needs_fresh_evidence() {
        let (_tmp, repo, git) = evidence_repo();
        let base1 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "task"]);
        std::fs::write(repo.join("Button.tsx"), "v1\n").unwrap();
        std::fs::write(repo.join("notes.md"), "mine\n").unwrap();
        let old_tree = crate::git::changes::worktree_tree(&repo).unwrap();
        git(&["checkout", "-q", "-b", "other", &base1]);
        std::fs::write(repo.join("notes.md"), "theirs\n").unwrap();
        git(&["add", "notes.md"]);
        git(&["commit", "-q", "-m", "landed"]);
        let base2 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "task"]);
        std::fs::write(repo.join("notes.md"), "merged\n").unwrap();
        assert_eq!(
            own_changes_since(&repo, &old_tree, Some(&base1), &base2).unwrap(),
            vec!["notes.md".to_string()]
        );
    }

    #[test]
    fn a_non_ui_source_edit_reruns_the_screenshot_and_a_test_edit_does_not() {
        let (_tmp, repo, git) = evidence_repo();
        let base = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "task"]);
        let old_tree = crate::git::changes::worktree_tree(&repo).unwrap();
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/render.rs"), "t\n").unwrap();
        assert!(own_changes_since(&repo, &old_tree, Some(&base), &base)
            .unwrap()
            .is_empty());
        std::fs::write(repo.join("render.rs"), "fn draw() {}\n").unwrap();
        assert_eq!(
            own_changes_since(&repo, &old_tree, Some(&base), &base).unwrap(),
            vec!["render.rs".to_string()]
        );
    }
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    #[test]
    fn new_answers_feed_lead_answers_but_not_lead_stops_to_the_criteria_check() {
        let mut task = task_with_status(TaskStatus::Queued);
        task.fixture_data().decisions = vec![
            "Lead: old question -> old answer (answered by a lead session)".into(),
            "Lead: q -> a (answered by a lead session)".into(),
            "Lead: stop the service? -> yes (answered by a lead session)".into(),
            "Lead: stop".into(),
            "Lead: dropped from parent".into(),
            "Owner: B".into(),
            "Policy: q -> a (policy: e)".into(),
        ];
        task.record_answer_progress(1);
        assert_eq!(
            new_answers(&task),
            [
                task.decisions[1].clone(),
                task.decisions[2].clone(),
                task.decisions[5].clone(),
                task.decisions[6].clone()
            ]
        );
    }
}
