use crate::engine::attempts::brief::build_brief;
use crate::engine::attempts::report::agent_decision_lines;
use crate::task::state::TaskStatus;

use super::{landed_dependencies_block, landed_entry, relay_block};
use crate::engine::attempts::test_fixtures::{dependent_of, done_dependency, sample_task};

#[test]
fn a_relay_brief_says_dependencies_are_on_the_branch_and_not_to_merge() {
    let dep = done_dependency("d1", 10);
    let mut task = dependent_of(&["d1"]);
    task.fixture_data().queue.relay_of = Some("d1".into());
    let block = relay_block(&task, &[dep]);
    assert!(block.contains("already on your branch"), "{block}");
    assert!(block.contains("do not merge, rebase"), "{block}");
}

#[test]
fn landed_dependencies_skip_a_dependency_that_is_not_done() {
    let mut dep = done_dependency("d1", 10);
    dep.fixture_data().status = TaskStatus::Running;
    let task = dependent_of(&["d1"]);
    assert_eq!(landed_dependencies_block(&task, &[dep.clone()], None), "");
    dep.fixture_data().status = TaskStatus::Done;
    let block = landed_dependencies_block(&task, &[dep], None);
    assert!(block.starts_with("## Landed dependencies"));
    assert!(block.contains("### d1\n"));
    assert!(block.contains("DEP_SUMMARY") && block.contains("DEP_HANDOFF"));
    assert!(block.contains("a.txt"));
}

#[test]
fn landed_dependencies_keep_the_last_five_agent_and_owner_decisions() {
    let mut dep = done_dependency("d1", 10);
    dep.fixture_data().decisions = vec!["Rebase: onto main".into(), "Planner: split".into()];
    for i in 0..7 {
        dep.fixture_data()
            .decisions
            .push(format!("Agent: choice {i}"));
    }
    dep.fixture_data()
        .decisions
        .push("Owner: final call".into());
    dep.fixture_data().decisions.push("Land: landed".into());
    let block = landed_dependencies_block(&dependent_of(&["d1"]), &[dep], None);
    assert!(!block.contains("Rebase:") && !block.contains("Planner:"));
    assert!(!block.contains("Land:"));
    assert!(!block.contains("choice 0") && !block.contains("choice 2"));
    for i in 3..7 {
        assert!(block.contains(&format!("Agent: choice {i}")));
    }
    assert!(block.contains("Owner: final call"));
    assert_eq!(block.matches("Agent: choice").count(), 4);
}

#[test]
fn landed_dependencies_list_twenty_files_then_the_rest_as_a_count() {
    let mut dep = done_dependency("d1", 10);
    dep.fixture_data().attempts[0].changed_files =
        (1..=25).map(|i| format!("f{i:02}.txt")).collect();
    let block = landed_dependencies_block(&dependent_of(&["d1"]), &[dep], None);
    assert_eq!(block.matches(".txt").count(), 20);
    assert!(block.contains("f20.txt") && !block.contains("f21.txt"));
    assert!(block.contains("+5 more"));
}

#[test]
fn landed_dependencies_clip_each_entry_and_keep_the_closing_fence() {
    let mut dep = done_dependency("d1", 10);
    dep.fixture_data().attempts[0].summary = Some("x".repeat(5000));
    let other = done_dependency("d2", 10);
    let block = landed_dependencies_block(&dependent_of(&["d1", "d2"]), &[dep, other], None);
    let first = block.split("### d2").next().unwrap();
    let entry = first.split("### d1\n").nth(1).unwrap().trim_end();
    assert!(entry.len() <= 1500, "entry was {} bytes", entry.len());
    assert!(entry.ends_with("</untrusted-data>"));
    assert_eq!(block.matches("</untrusted-data>").count(), 2);
}

#[test]
fn landed_dependencies_clip_multibyte_text_within_the_cap() {
    let mut dep = done_dependency("d1", 10);
    dep.fixture_data().attempts[0].summary = Some("\u{1F642}".repeat(1000));
    let block = landed_dependencies_block(&dependent_of(&["d1"]), &[dep], None);
    let entry = block.split("### d1\n").nth(1).unwrap().trim_end();
    assert!(entry.chars().count() <= 1500);
    assert!(entry.len() <= 1500, "entry was {} bytes", entry.len());
    assert!(entry.ends_with("</untrusted-data>"));
}

#[test]
fn landed_dependencies_only_include_those_that_ended_after_landed_after() {
    let old = done_dependency("old", 10);
    let new = done_dependency("new", 30);
    let task = dependent_of(&["old", "new"]);
    let all = [old, new];
    let both = landed_dependencies_block(&task, &all, None);
    assert!(both.contains("### old") && both.contains("### new"));
    let since = landed_dependencies_block(&task, &all, Some(20));
    assert!(!since.contains("### old") && since.contains("### new"));
    assert_eq!(landed_dependencies_block(&task, &all, Some(30)), "");
}

#[test]
fn a_parent_dependency_shows_only_its_title_and_decisions() {
    let mut dep = done_dependency("d1", 10);
    dep.fixture_data().attempts.clear();
    dep.fixture_data().decisions = vec!["Agent: split it".into()];
    let block = landed_dependencies_block(&dependent_of(&["d1"]), &[dep], None);
    assert!(block.contains("Dependency d1") && block.contains("Agent: split it"));
    assert!(!block.contains("Summary:") && !block.contains("Changed files"));
}

#[test]
fn lead_decisions_are_kept_in_dependency_briefs_and_the_echoed_prefix_is_stripped() {
    let mut task = sample_task();
    task.fixture_data().decisions = vec!["Lead: q -> a (answered by a lead session)".into()];
    assert!(landed_entry(&task).contains(&task.decisions[0]));
    assert!(build_brief(&task, "", "").contains(&task.decisions[0]));
    assert_eq!(agent_decision_lines(&["Lead: a".into()]), ["Agent: a"]);
}
