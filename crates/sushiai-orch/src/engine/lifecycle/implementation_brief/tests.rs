use super::build;
use crate::engine::attempts::context::parent_failure_block;
use crate::engine::attempts::resume::RunBrief;
use crate::engine::lifecycle::selection::Selection;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::state::TaskStatus;

#[test]
fn a_parent_carry_conflict_uses_only_the_base_moved_block() {
    let (app, _dir) = test_app();
    let route = app.settings.read().unwrap().routes[0].clone();
    let detail = "The parent could not carry its changed files";
    for conflicted in [false, true] {
        let mut task = task_with_status(TaskStatus::Running);
        if conflicted {
            task.record_carry_conflicts(vec!["src/conflicted.rs".into()]);
        }
        let full = build(&app, &task, Some(detail), ("", ""));
        let selection = Selection {
            route: route.clone(),
            carry_conflicts: task.queue.carry_conflicts.clone(),
        };
        let brief = RunBrief::build(&mut task, &selection, full, false);
        for text in [&brief.current, &brief.full] {
            if conflicted {
                assert!(text.contains("## Base moved"), "{text}");
                assert!(text.contains("src/conflicted.rs"), "{text}");
                assert!(!text.contains(&parent_failure_block(detail)), "{text}");
                assert!(!text.contains(detail), "{text}");
            } else {
                assert!(text.contains(&parent_failure_block(detail)), "{text}");
                assert!(!text.contains("## Base moved"), "{text}");
            }
        }
    }
}
