use super::{compiled, TaskExit, TaskFlow, TaskNext, TaskOutcome, TaskStep};

const YAML: &str = include_str!("../../../../pipelines/task.yaml");

#[test]
fn the_compiled_attempt_routes_each_produced_receipt_to_its_consumer() {
    let flow = compiled();
    let mut step = flow.entry();
    let mut trace = vec![step];
    while step != TaskStep::Complete {
        let TaskNext::Step(next) = flow.next(step, TaskOutcome::Ready) else {
            panic!("a ready receipt ended the flow");
        };
        trace.push(next);
        step = next;
    }
    assert_eq!(
        trace,
        [
            TaskStep::Execute,
            TaskStep::Settle,
            TaskStep::Inspect,
            TaskStep::Qualify,
            TaskStep::Review,
            TaskStep::Complete
        ]
    );
    assert_eq!(
        flow.next(step, TaskOutcome::Finished),
        TaskNext::Exit(TaskExit::Finish)
    );
    assert_eq!(
        flow.next(step, TaskOutcome::Continue),
        TaskNext::Exit(TaskExit::Retry)
    );
}

#[test]
fn changed_stopped_and_failed_operations_cannot_consume_later_receipts() {
    let flow = compiled();
    for (step, outcome, exit) in [
        (TaskStep::Execute, TaskOutcome::Retry, TaskExit::Retry),
        (TaskStep::Execute, TaskOutcome::Finished, TaskExit::Finish),
        (TaskStep::Settle, TaskOutcome::Changed, TaskExit::Retry),
        (TaskStep::Settle, TaskOutcome::Stopped, TaskExit::Stop),
        (TaskStep::Inspect, TaskOutcome::Failed, TaskExit::Fail),
        (TaskStep::Qualify, TaskOutcome::Changed, TaskExit::Retry),
        (TaskStep::Review, TaskOutcome::Stopped, TaskExit::Stop),
    ] {
        assert_eq!(flow.next(step, outcome), TaskNext::Exit(exit));
    }
}

#[test]
fn missing_and_duplicate_outcome_edges_are_rejected() {
    let edge = "  - { from: execute, on: ready, to: settle }\n";
    assert!(TaskFlow::parse(&YAML.replace(edge, ""))
        .unwrap_err()
        .contains("missing"));
    assert!(TaskFlow::parse(&format!("{YAML}{edge}"))
        .unwrap_err()
        .contains("duplicate"));
}

#[test]
fn unknown_fields_steps_and_malformed_yaml_are_rejected() {
    for text in [
        YAML.replace("entry: execute", "entry: unknown"),
        format!("{YAML}command: something\n"),
        YAML.replace("from: execute", "from: ["),
    ] {
        assert!(TaskFlow::parse(&text).is_err());
    }
}

#[test]
fn yaml_cannot_skip_verification_or_turn_stop_into_an_operation() {
    for text in [
        YAML.replace("entry: execute", "entry: review"),
        YAML.replace(
            "from: inspect, on: ready, to: qualify",
            "from: inspect, on: ready, to: review",
        ),
        YAML.replace(
            "from: settle, on: stopped, to: stop",
            "from: settle, on: stopped, to: inspect",
        ),
        YAML.replace(
            "from: review, on: changed, to: retry",
            "from: review, on: changed, to: complete",
        ),
    ] {
        assert!(TaskFlow::parse(&text).is_err());
    }
}

#[test]
fn transition_declaration_order_does_not_change_receipt_routing() {
    let mut lines = YAML.lines().skip(2).collect::<Vec<_>>();
    lines.reverse();
    let flow = TaskFlow::parse(&format!(
        "entry: execute\ntransitions:\n{}\n",
        lines.join("\n")
    ))
    .unwrap();
    assert_eq!(
        flow.next(TaskStep::Qualify, TaskOutcome::Ready),
        TaskNext::Step(TaskStep::Review)
    );
}
