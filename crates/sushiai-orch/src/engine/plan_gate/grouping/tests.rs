use super::*;
use crate::engine::plan_gate::test_support::part;

#[test]
fn plan_gate_merges_overlapping_and_checkless_parts_and_keeps_the_rest() {
    let parts = vec![
        part("a", &[], &["src"], "check a"),
        part("b", &[], &["src/b.rs"], "check b"),
        part("c", &[], &["docs"], "check c"),
        part("d", &[], &[], "check d"),
        part("e", &["c"], &["web"], ""),
    ];
    let plan = merge_parts(&parts);
    assert_eq!(plan.separate, vec![2]);
    assert_eq!(plan.merged, vec![0, 1, 3, 4]);
}

#[test]
fn complete_overlapping_drafts_keep_children_but_incomplete_overlaps_merge() {
    let mut a = part("a", &[], &["src/shared"], "");
    a.goal = "Add A".to_string();
    a.criteria = vec!["A is present".to_string()];
    a.verify = vec!["test -f src/shared/a".to_string()];
    let mut b = part("b", &[], &["src/shared"], "");
    b.goal = "Add B".to_string();
    b.criteria = vec!["B is present".to_string()];
    b.verify = vec!["test -f src/shared/b".to_string()];
    assert_eq!(
        merge_parts(&[a.clone(), b.clone()]),
        MergePlan {
            separate: vec![0, 1],
            merged: vec![],
        }
    );
    b.goal.clear();
    assert_eq!(
        merge_parts(&[a, b]),
        MergePlan {
            separate: vec![],
            merged: vec![0, 1],
        }
    );
}

#[test]
fn plan_gate_orders_the_group_by_dependencies_and_keeps_planner_order_otherwise() {
    let parts = vec![
        part("a", &["c"], &["src"], "x"),
        part("b", &[], &["src"], "x"),
        part("c", &[], &["src"], "x"),
    ];
    assert_eq!(merge_parts(&parts).merged, vec![1, 2, 0]);
}

#[test]
fn plan_gate_merges_everything_when_the_group_would_make_a_cycle() {
    // b is separate and depends on a (grouped); a depends on b through c.
    let parts = vec![
        part("a", &["b"], &[], "x"),
        part("b", &["a"], &["docs"], "x"),
    ];
    // a and b depend on each other: not a valid graph, but the group
    // mapping alone must not loop.
    let plan = merge_parts(&parts);
    assert!(plan.separate.is_empty());
    assert_eq!(plan.merged.len(), 2);

    let parts = vec![
        part("a", &[], &[], "x"),
        part("b", &["a"], &["docs"], "x"),
        part("c", &["b"], &[], "x"),
        part("d", &["b"], &["web"], "x"),
    ];
    // Group {a, c} depends on b (via c) and b depends on the group (via a).
    let plan = merge_parts(&parts);
    assert!(plan.separate.is_empty());
    assert_eq!(plan.merged, vec![0, 1, 2, 3]);
}
