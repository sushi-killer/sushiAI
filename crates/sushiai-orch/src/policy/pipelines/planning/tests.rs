use super::*;

#[test]
fn the_compiled_plan_has_every_typed_branch_and_one_cached_definition() {
    let flow = PlanningFlow::compiled().unwrap();
    assert!(std::ptr::eq(flow, PlanningFlow::compiled().unwrap()));
    assert_eq!(flow.entry(), PlanningStep::Prepare);
    assert_eq!(flow.transitions.len(), 20);
    for step in PlanningStep::ALL {
        for outcome in step.outcomes() {
            assert!(flow.next(step, *outcome).is_ok(), "{step:?}/{outcome:?}");
        }
    }
    assert!(flow
        .next(PlanningStep::Return, PlanningOutcome::Ready)
        .is_err());
}

#[test]
fn every_missing_or_duplicate_branch_is_rejected() {
    let original = PlanningFlow::parse(SOURCE).unwrap();
    for index in 0..original.transitions.len() {
        let mut missing = PlanningFlow::parse(SOURCE).unwrap();
        missing.transitions.remove(index);
        assert!(missing.validate().is_err(), "missing edge {index}");
        let mut duplicate = PlanningFlow::parse(SOURCE).unwrap();
        let edge = &original.transitions[index];
        duplicate.transitions.push(PlanningTransition {
            from: edge.from,
            on: edge.on,
            to: edge.to,
        });
        assert!(duplicate.validate().is_err(), "duplicate edge {index}");
    }
}

#[test]
fn typed_inputs_and_gate_order_reject_every_other_destination() {
    let original = PlanningFlow::parse(SOURCE).unwrap();
    for index in 0..original.transitions.len() {
        for target in PlanningStep::ALL {
            let mut changed = PlanningFlow::parse(SOURCE).unwrap();
            changed.transitions[index].to = target;
            assert_eq!(
                changed.validate().is_ok(),
                target == original.transitions[index].to,
                "{:?}/{:?} -> {target:?}",
                original.transitions[index].from,
                original.transitions[index].on
            );
        }
    }
}

#[test]
fn unknown_operations_fields_outcomes_and_versions_are_rejected_at_decode() {
    for text in [
        SOURCE.replace("entry: prepare", "entry: finish"),
        SOURCE.replace("version: 1", "version: 2"),
        SOURCE.replace("to: review", "to: execute_shell"),
        SOURCE.replace("on: malformed", "on: ignore_error"),
        format!("{SOURCE}\ncommand: execute-anything\n"),
        SOURCE.replace(
            "on: ready, to: draft",
            "on: ready, to: draft, command: ignored",
        ),
        "version: [1]\nentry: prepare\ntransitions: []".into(),
    ] {
        assert!(PlanningFlow::parse(&text).is_err(), "accepted {text}");
    }
}

#[test]
fn a_structurally_valid_yaml_mapping_cannot_omit_the_required_review_gate() {
    let bypass = SOURCE.replace(
        "from: draft, on: ready, to: review",
        "from: draft, on: ready, to: questions",
    );
    assert!(PlanningFlow::parse(&bypass).is_err());
    let missing = SOURCE
        .lines()
        .filter(|line| !line.contains("from: review, on: retry"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(PlanningFlow::parse(&missing).is_err());
    let duplicate = format!("{SOURCE}\n  - {{from: prepare, on: ready, to: draft}}\n");
    assert!(PlanningFlow::parse(&duplicate).is_err());
}
