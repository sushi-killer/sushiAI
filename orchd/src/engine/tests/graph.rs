fn graph_task(id: &str, status: TaskStatus, depends_on: &[&str], parent: Option<&str>) -> Task {
    let mut t = task_with_status(status);
    t.id = id.to_string();
    t.depends_on = depends_on.iter().map(|d| d.to_string()).collect();
    t.parent = parent.map(str::to_string);
    t
}

#[test]
fn a_parent_waiting_for_a_child_that_depends_on_it_is_a_cycle() {
    let tasks = vec![
        graph_task("p", TaskStatus::Queued, &[], None),
        graph_task("c", TaskStatus::Queued, &["p"], Some("p")),
    ];
    assert!(has_cycle(&wait_edges(&tasks)));
    let tasks = vec![
        graph_task("p", TaskStatus::Queued, &[], None),
        graph_task("a", TaskStatus::Queued, &[], Some("p")),
        graph_task("b", TaskStatus::Queued, &["a"], Some("p")),
    ];
    assert!(!has_cycle(&wait_edges(&tasks)));
}

#[test]
fn waits_state_reads_dependencies_and_children() {
    let mut tasks = vec![
        graph_task("p", TaskStatus::Running, &[], None),
        graph_task("a", TaskStatus::Done, &[], Some("p")),
        graph_task("b", TaskStatus::Queued, &["a", "gone"], Some("p")),
    ];
    assert_eq!(waits_state(&tasks[2], &tasks), Waits::Ready);
    assert_eq!(waits_state(&tasks[0], &tasks), Waits::Pending);
    assert_eq!(waits_state(&tasks[1], &tasks), Waits::Nothing);
    tasks[2].status = TaskStatus::Failed;
    assert_eq!(
        waits_state(&tasks[0], &tasks),
        Waits::Ended(vec!["b".into()])
    );
    // An archived child no longer holds its parent back.
    tasks[2].archived = true;
    assert_eq!(waits_state(&tasks[0], &tasks), Waits::Ready);
}
