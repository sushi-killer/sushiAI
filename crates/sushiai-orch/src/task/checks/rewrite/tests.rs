use super::super::authority::criterion_commands;
use super::*;
use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, Baseline, Check, TaskStatus};

fn sourced(task: &mut Task, command: &str, source: CheckSource) {
    task.fixture_data()
        .planner_tests
        .sources
        .push(SourcedCheck {
            command: command.to_string(),
            source,
        });
}

fn rewrite(task: &mut Task, index: usize, rewrite: &CheckRewrite) -> Result<(), String> {
    let edit = CriterionEdit::capture(task, index).unwrap();
    replace(&CheckAuthority::default(), task, &edit, rewrite)
}

fn rewritten_save_check() -> (Task, &'static str) {
    let mut task = sample_task("rewrite", TaskStatus::Waiting, AttemptStatus::Running);
    task.fixture_data().criteria = vec![
        "zero".into(),
        "Saving works -- check: node --test tests/save.test.cjs".into(),
    ];
    task.fixture_data().visual_criteria = vec![task.criteria[1].clone()];
    task.fixture_data().verify = vec!["true".into(), "node --test tests/save.test.cjs".into()];
    task.fixture_data().checks = vec![
        Check {
            criterion: 0,
            run: "test -f zero.txt".into(),
            baseline: Some(Baseline::Fail),
        },
        Check {
            criterion: 1,
            run: "node --test tests/save.test.cjs".into(),
            baseline: Some(Baseline::Fail),
        },
    ];
    task.fixture_data().held_out = Some(Check {
        criterion: 1,
        run: "grep -q secret-held-out out.txt".into(),
        baseline: Some(Baseline::Fail),
    });
    for c in [
        "test -f zero.txt",
        "node --test tests/save.test.cjs",
        "grep -q secret-held-out out.txt",
    ] {
        sourced(&mut task, c, CheckSource::Planner);
    }
    let new = "grep -q saved settings.json";
    rewrite(
        &mut task,
        1,
        &CheckRewrite {
            command: (new).to_string(),
            criterion_text: ("").to_string(),
            reason: ("the test file was renamed").to_string(),
        },
    )
    .unwrap();
    (task, new)
}

#[test]
fn a_rewrite_replaces_every_check_of_the_criterion_and_records_it_as_the_planners() {
    let (task, new) = rewritten_save_check();
    assert_eq!(task.criteria[1], format!("Saving works -- check: {new}"));
    assert_eq!(task.visual_criteria, vec![task.criteria[1].clone()]);
    assert_eq!(task.verify, vec!["true".to_string(), new.to_string()]);
    let runs: Vec<(usize, &str)> = task
        .checks
        .iter()
        .map(|c| (c.criterion, c.run.as_str()))
        .collect();
    assert_eq!(runs, vec![(0, "test -f zero.txt"), (1, new)]);
    assert_eq!(task.checks[1].baseline, None);
    assert_eq!(task.held_out.as_ref().unwrap().run, new);
    assert!(!CheckAuthority::default().is_shared(&task, new));
    assert_eq!(criterion_commands(&task, 1), vec![new, new, new]);
    let line = task.decisions.last().unwrap();
    assert!(
        line.contains("node --test tests/save.test.cjs")
            && line.contains(new)
            && line.contains("the test file was renamed")
            && line.contains("held-out check (criterion 1)")
            && !line.contains("secret-held-out"),
        "{line}"
    );
}

#[test]
fn a_rewrite_rewords_the_criterion_and_refuses_empty_or_identical_commands() {
    let (mut task, _) = rewritten_save_check();
    // The judge's wording replaces the prose; the check part stays the new command.
    rewrite(
        &mut task,
        1,
        &CheckRewrite {
            command: ("test -s settings.json").to_string(),
            criterion_text: ("Save writes the file -- check: x").to_string(),
            reason: ("r").to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        task.criteria[1],
        "Save writes the file -- check: test -s settings.json"
    );
    // An empty command or the same command changes nothing.
    let before = task.clone();
    assert!(rewrite(
        &mut task,
        1,
        &CheckRewrite {
            command: (" ").to_string(),
            criterion_text: ("").to_string(),
            reason: ("r").to_string()
        }
    )
    .is_err());
    assert!(rewrite(
        &mut task,
        0,
        &CheckRewrite {
            command: ("test -f zero.txt").to_string(),
            criterion_text: ("").to_string(),
            reason: ("r").to_string()
        }
    )
    .is_err());
    assert_eq!(task.checks, before.checks);
    assert_eq!(task.decisions, before.decisions);
}

#[test]
fn a_rewrite_rejects_old_baseline_observations_and_rechecks_the_new_command() {
    use crate::task::checks::baseline::{BaselineObservation, BaselineTicket};
    let mut task = sample_task("rewrite", TaskStatus::Waiting, AttemptStatus::Running);
    task.fixture_data().criteria = vec!["Keep the API -- check: test -f old.txt".into()];
    task.fixture_data().checks = vec![Check {
        criterion: 0,
        run: "test -f old.txt".into(),
        baseline: None,
    }];
    sourced(&mut task, "test -f old.txt", CheckSource::Planner);
    let old = BaselineTicket::capture(&task);
    rewrite(
        &mut task,
        0,
        &CheckRewrite {
            command: "test -f new.txt".into(),
            criterion_text: String::new(),
            reason: "renamed".into(),
        },
    )
    .unwrap();
    assert_eq!(task.checks[0].run, "test -f new.txt");
    assert_eq!(task.checks[0].baseline, None);
    assert!(!crate::task::checks::baseline::record(
        &mut task,
        &old,
        &[BaselineObservation {
            baseline: Baseline::Pass,
            code: Some(0)
        }],
        1
    ));
    assert_eq!(task.checks[0].baseline, None);
    let new = BaselineTicket::capture(&task);
    assert_eq!(new.todo, vec![(Some(0), 0, "test -f new.txt".into())]);
}

#[test]
fn a_rewrite_preserves_owner_checks_and_refuses_a_changed_criterion_contract() {
    let mut task = sample_task("rewrite", TaskStatus::Waiting, AttemptStatus::Running);
    task.fixture_data().criteria = vec!["Keep the API -- check: test -f api.txt".into()];
    task.fixture_data().checks = vec![Check {
        criterion: 0,
        run: "test -f api.txt".into(),
        baseline: Some(Baseline::Fail),
    }];
    sourced(&mut task, "test -f api.txt", CheckSource::Owner);
    let edit = CriterionEdit::capture(&task, 0).unwrap();
    let requested = CheckRewrite {
        command: "true".into(),
        criterion_text: String::new(),
        reason: "changed".into(),
    };
    let authority = CheckAuthority::default();
    let before = serde_json::to_value(&task).unwrap();
    assert!(replace(&authority, &mut task, &edit, &requested).is_err());
    assert_eq!(serde_json::to_value(&task).unwrap(), before);
    task.fixture_data().criteria[0] = "A new owner's criterion".into();
    let before = serde_json::to_value(&task).unwrap();
    assert!(replace(&authority, &mut task, &edit, &requested).is_err());
    assert_eq!(serde_json::to_value(&task).unwrap(), before);
}
