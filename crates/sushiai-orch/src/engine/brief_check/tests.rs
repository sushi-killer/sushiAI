use super::refresh::refresh_after_dependencies;
use super::reply::{build_check_brief, parse_missing_tool, parse_verdict, Verdict};
use super::run::{check_brief, BriefAction};
use crate::engine::test_support::{assert_helper_timeout, slow_helper_task};
use crate::harness::cancel::CancelToken;
use crate::task::{Task, TaskStatus};
use serde_json::json;
#[cfg(test)]
mod cases {
    use super::*;

    #[test]
    fn a_verdict_is_read_from_json_inside_prose_or_a_fence() {
        let text = "```json\n{\"contradiction\": true, \"conflict\": \"A vs B\"}\n```";
        assert!(matches!(parse_verdict(text), Verdict::Conflict(c) if c == "A vs B"));
        assert!(matches!(
            parse_verdict("{\"contradiction\": false, \"conflict\": \"\"}"),
            Verdict::Clear
        ));
    }

    #[test]
    fn a_reply_without_a_flag_is_unusable() {
        assert!(matches!(parse_verdict("looks fine"), Verdict::Unusable(_)));
        assert!(matches!(
            parse_verdict("{\"conflict\": \"x\"}"),
            Verdict::Unusable(_)
        ));
    }

    #[test]
    fn a_contradiction_without_text_still_counts() {
        assert!(matches!(
            parse_verdict("{\"contradiction\": true}"),
            Verdict::Conflict(c) if !c.is_empty()
        ));
    }

    #[test]
    fn the_check_brief_carries_goal_and_criteria_as_json() {
        let b = build_check_brief("g", &["a".to_string()], &json!({"mcp": []}));
        assert!(b.starts_with("You check a coding task brief"));
        // Key order depends on serde_json's `preserve_order` (enabled by another workspace
        // crate): check each member, not the whole object.
        for part in [
            r#""criteria":["a"]"#,
            r#""goal":"g""#,
            r#""tools":{"mcp":[]}"#,
        ] {
            assert!(b.contains(part), "{part}");
        }
    }

    #[test]
    fn a_missing_tool_is_read_from_the_reply_and_checked_against_the_brief() {
        let off = vec![("asana".to_string(), "Asana".to_string())];
        let reply = r#"{"contradiction": false, "conflict": "", "missingTool": {"criterion": 1, "capability": " move an  Asana task ", "toolId": "asana"}}"#;
        let m = parse_missing_tool(reply, 2, &off).unwrap();
        assert_eq!(m.criterion, 1);
        assert_eq!(m.capability, "move an Asana task");
        assert_eq!(m.tool_id.as_deref(), Some("asana"));
        // An id that is not an off tool of this task is dropped, the claim kept.
        let other = reply.replace("\"asana\"}", "\"jira\"}");
        assert_eq!(parse_missing_tool(&other, 2, &off).unwrap().tool_id, None);
        // A criterion that does not exist, null and blank claims are nothing.
        assert!(parse_missing_tool(reply, 1, &off).is_none());
        assert!(parse_missing_tool(r#"{"missingTool": null}"#, 2, &off).is_none());
        assert!(parse_missing_tool(
            r#"{"missingTool": {"criterion": 0, "capability": " "}}"#,
            2,
            &off
        )
        .is_none());
    }

    #[tokio::test]
    async fn a_split_parent_dependency_triggers_the_refresh_from_its_landed_diff() {
        use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
        let (app, dir) = test_app();
        app.settings.write().unwrap().brief_check_route = String::new();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        init_git_repo(&repo);
        std::fs::write(repo.join("src/touched.txt"), "x\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "landed"]);
        let landed = git_in(&repo, &["rev-parse", "HEAD"]);
        // A split parent: no implement attempt of its own, only a landing.
        let mut dep = task_with_status(TaskStatus::Done);
        dep.fixture_data().repo = repo.display().to_string();
        dep.fixture_data().landed_sha = Some(landed);
        app.store.save_task(&dep).unwrap();

        let child_with = |paths: &[&str]| {
            let mut t = task_with_status(TaskStatus::Queued);
            t.fixture_data().repo = repo.display().to_string();
            t.fixture_data().planned_by_parent = true;
            t.record_dependencies(vec![dep.id.clone()]);
            t.record_paths(paths.iter().map(|p| p.to_string()).collect());
            app.store.save_task(&t).unwrap();
            t
        };
        let hit = child_with(&["src/touched.txt"]);
        let miss = child_with(&["src/other.txt"]);

        for t in [&hit, &miss] {
            assert_eq!(
                refresh_after_dependencies(&app, &t.id, &CancelToken::new()).await,
                Ok(())
            );
        }
        let refresh_lines = |t: &Task| {
            app.store
                .load_task(&t.id)
                .unwrap()
                .unwrap()
                .decisions
                .iter()
                .filter(|d| d.starts_with("Refresh:"))
                .count()
        };
        assert_eq!(
            refresh_lines(&hit),
            1,
            "the touched part starts the refresh"
        );
        assert_eq!(refresh_lines(&miss), 0);
        assert!(
            app.store
                .load_task(&miss.id)
                .unwrap()
                .unwrap()
                .brief_check
                .refreshed
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn brief_check_timed_out_continues_without_a_usable_answer() {
        let (app, _dir, task, _) = slow_helper_task();
        assert!(matches!(
            check_brief(&app, &task.id, false, &CancelToken::new()).await,
            BriefAction::Proceed
        ));
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(task.brief_check.done);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.starts_with("Brief check: no usable answer") && d.ends_with("continuing")));
        assert_helper_timeout(&task, "brief-check");
        let cost = crate::store::costs::read_all(&app.data_dir);
        assert_eq!(cost.len(), 1);
        assert!(cost[0].attempt.is_none());
    }
}
