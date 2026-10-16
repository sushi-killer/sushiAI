use super::{DeliveryFlow, DeliveryOutcome, DeliveryStep};

const YAML: &str = include_str!("../../../../pipelines/delivery.yaml");

#[test]
fn compiled_delivery_routes_each_operation_receipt() {
    let flow = DeliveryFlow::compiled();
    assert_eq!(flow.entry(), DeliveryStep::Admit);
    let cases = [
        (
            DeliveryStep::Admit,
            DeliveryOutcome::Admitted,
            DeliveryStep::Land,
        ),
        (
            DeliveryStep::Land,
            DeliveryOutcome::Unlanded,
            DeliveryStep::Commit,
        ),
        (
            DeliveryStep::Land,
            DeliveryOutcome::Waiting,
            DeliveryStep::Wait,
        ),
        (
            DeliveryStep::Land,
            DeliveryOutcome::Landed,
            DeliveryStep::Landed,
        ),
        (
            DeliveryStep::Land,
            DeliveryOutcome::Failed,
            DeliveryStep::Retry,
        ),
        (
            DeliveryStep::Land,
            DeliveryOutcome::Rereview,
            DeliveryStep::Review,
        ),
        (
            DeliveryStep::Land,
            DeliveryOutcome::Cancelled,
            DeliveryStep::Cancel,
        ),
    ];
    for (from, outcome, next) in cases {
        assert_eq!(flow.next(from, outcome), next);
    }
}

fn rejects_change(before: &str, after: &str) {
    let changed = YAML.replace(before, after);
    assert_ne!(changed, YAML, "the fixture must change an existing edge");
    assert!(DeliveryFlow::parse(&changed).is_err(), "{changed}");
}

#[test]
fn invalid_edges_cannot_skip_admission_or_use_an_incompatible_receipt() {
    rejects_change("entry: admit", "entry: land");
    rejects_change("on: admitted, to: land", "on: admitted, to: commit");
    for outcome in ["waiting", "landed", "failed", "rereview", "cancelled"] {
        let from = match outcome {
            "waiting" => "wait",
            "landed" => "landed",
            "failed" => "retry",
            "rereview" => "review",
            _ => "cancel",
        };
        rejects_change(
            &format!("on: {outcome}, to: {from}"),
            &format!("on: {outcome}, to: commit"),
        );
    }
    rejects_change("on: unlanded, to: commit", "on: unlanded, to: land");
}

#[test]
fn incomplete_duplicate_or_terminal_outcomes_are_rejected() {
    let missing = YAML
        .lines()
        .filter(|line| !line.contains("on: failed"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(DeliveryFlow::parse(&missing).is_err());
    for extra in [
        "  - { from: admit, on: admitted, to: land }\n",
        "  - { from: commit, on: admitted, to: land }\n",
        "  - { from: land, on: admitted, to: land }\n",
    ] {
        assert!(DeliveryFlow::parse(&format!("{YAML}{extra}")).is_err());
    }
}

#[test]
fn delivery_yaml_rejects_unknown_fields_operations_and_runtime_commands() {
    for extra in [
        "command: echo bypass\n",
        "expression: true\n",
        "override: owner\n",
    ] {
        assert!(DeliveryFlow::parse(&format!("{extra}{YAML}")).is_err());
    }
    rejects_change("to: commit", "to: run_command");
    rejects_change("on: unlanded", "on: anything");
    rejects_change("from: admit,", "from: admit, command: echo bypass,");
    assert!(DeliveryFlow::parse(&format!("entry: admit\n{YAML}")).is_err());
}
