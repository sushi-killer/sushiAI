use crate::delivery::pr::body::pr_body;
use crate::delivery::pr::body::waived_section;
use crate::delivery::pr::body::without_waived_section;
use crate::delivery::pr::body::UNREVIEWED_LINE;
use crate::delivery::pr::branch::check_pr_view;
use crate::delivery::pr::branch::pushed_head_mismatch;
use crate::delivery::pr::error::PrError;
use crate::delivery::pr::error::PrRefusal;
use crate::engine::test_support::task_with_status;
use crate::task::lifecycle::UnreviewedPart;
use crate::task::questions::Assumption;
use crate::task::questions::QuestionKind;
use crate::task::questions::WAIVED;
use crate::task::questions::WAIVER_BY;
use crate::task::state::TaskStatus;

#[cfg(test)]
mod refusal_tests {
    use super::*;

    #[test]
    fn an_orchd_refusal_is_permanent_even_when_its_text_holds_a_transient_marker() {
        let refusal = PrRefusal::CarryFailed("could not reach origin; try again later".into());
        assert!(refusal.permanent());
        assert!(PrRefusal::BadBranch("timed out".into()).permanent());
        assert!(PrRefusal::Contained("rate limit".into()).permanent());
    }

    #[test]
    fn a_parent_pr_names_its_unreviewed_subtasks() {
        let mut child = task_with_status(TaskStatus::Done);
        child.fixture_data().title = "Part".into();
        let mut parent = task_with_status(TaskStatus::Done);
        parent.fixture_data().goal = "Whole".into();
        parent.fixture_data().unreviewed = true;
        parent
            .fixture_data()
            .loop_state
            .unreviewed_parts
            .push(UnreviewedPart {
                id: child.id.clone(),
                title: child.title.clone(),
            });
        let body = pr_body(&parent);
        assert!(body.starts_with(UNREVIEWED_LINE), "{body}");
        assert!(body.contains("subtask \"Part\""), "{body}");
    }

    #[test]
    fn only_a_busy_pull_request_may_pass_on_its_own() {
        assert!(!PrRefusal::InFlight.permanent());
        assert!(PrRefusal::NotDone.permanent());
        assert!(PrRefusal::OverseerOwns.permanent());
    }

    #[test]
    fn a_refusal_keeps_the_owner_message() {
        let open = PrRefusal::AlreadyOpen {
            on: "feature/a".into(),
            url: "https://github.example/acme/app/pull/7".into(),
        };
        assert_eq!(
            open.to_string(),
            "a pull request is already open for this task on feature/a: https://github.example/acme/app/pull/7"
        );
        assert_eq!(
            PrError::from(PrRefusal::NotDone).to_string(),
            "only a done task can open a pull request"
        );
    }

    #[test]
    fn a_push_whose_remote_sha_differs_is_a_mismatch() {
        let pushed = "a".repeat(40);
        let same = format!("{pushed}\trefs/heads/feature/x\n");
        assert_eq!(pushed_head_mismatch("feature/x", &pushed, &same), None);
        let other = format!("{}\trefs/heads/feature/x\n", "b".repeat(40));
        let why = pushed_head_mismatch("feature/x", &pushed, &other).unwrap();
        assert!(why.contains("did not land"), "{why}");
        let gone = pushed_head_mismatch("feature/x", &pushed, "").unwrap();
        assert!(gone.contains("origin has no feature/x"), "{gone}");
    }

    #[test]
    fn a_pull_request_must_read_back_at_the_pushed_head_and_the_sent_title() {
        let url = "https://github.example/acme/app/pull/7";
        let view = |head: &str, title: &str| {
            format!(r#"{{"url":"{url}","headRefOid":"{head}","title":"{title}"}}"#)
        };
        assert!(check_pr_view(&view("abc", "t"), url, "abc", Some("t")).is_ok());
        assert!(check_pr_view(&view("abc", "old"), url, "abc", None).is_ok());
        let head = check_pr_view(&view("zzz", "t"), url, "abc", None).unwrap_err();
        assert!(head.contains("not the pushed abc"), "{head}");
        let title = check_pr_view(&view("abc", "old"), url, "abc", Some("t")).unwrap_err();
        assert!(title.contains("title"), "{title}");
        assert!(check_pr_view("", url, "abc", None).is_err());
    }

    #[test]
    fn a_waived_base_failure_is_listed_until_the_owner_overturns_it() {
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
        let body = pr_body(&task);
        assert!(body.contains("## Waived base failures"), "{body}");
        assert!(body.contains("- check: alpha fails on the base"), "{body}");
        task.fixture_data().assumptions[0].overturned = true;
        assert!(!pr_body(&task).contains("Waived"));
    }

    #[test]
    fn a_drafted_body_gets_the_waived_section_by_code_and_only_once() {
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
        let section = waived_section(&task);
        assert!(section.contains("- check: alpha fails on the base"));
        let own = "Intro\n\n## Waived base failures\n\n- invented\n\n## Notes\n\nkept\n";
        let stripped = without_waived_section(own);
        assert!(!stripped.contains("invented") && !stripped.contains("Waived"));
        assert!(stripped.contains("Intro") && stripped.contains("## Notes\n\nkept"));
        assert_eq!(without_waived_section("plain\n"), "plain\n");
    }
}
