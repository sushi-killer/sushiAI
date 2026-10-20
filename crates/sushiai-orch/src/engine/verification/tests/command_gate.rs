use crate::engine::review_findings::handle_repeated_findings;
use crate::engine::test_support::{task_with_status, test_app};
use crate::harness::cancel::CancelToken;
use crate::task::{
    Baseline, FindingDetail, FindingRecord, FindingScenario, FindingStatus, ReviewResult, Task,
    TaskStatus, Verdict,
};
use crate::verify::command_gate::command_refusal;
use crate::verify::options::split_verify_commands;
#[test]
fn the_check_gate_accepts_commands_and_refuses_prose() {
    let tmp = tempfile::tempdir().unwrap();
    let accepted = [
        "cargo test -p x foo",
        "npm test",
        "node scripts/a.mjs",
        "FOO=1 sh -c true",
        "./scripts/created-by-the-task.sh",
    ];
    let refused = [
        "Amendment{criteria: Some(vec![...])}.apply(&mut task)",
        "the parser keeps the last row",
        "--exact foo",
    ];
    for cmd in accepted {
        assert_eq!(command_refusal(tmp.path(), cmd), None, "{cmd}");
    }
    for cmd in refused {
        assert!(command_refusal(tmp.path(), cmd).is_some(), "{cmd}");
    }
    assert!(command_refusal(tmp.path(), "/no/such/absolute/program").is_some());
    let all: Vec<String> = accepted
        .iter()
        .chain(&refused)
        .map(|s| s.to_string())
        .collect();
    let (run, judged) = split_verify_commands(tmp.path(), &all);
    assert_eq!(run, accepted);
    assert_eq!(judged, refused);
}

fn repeated_finding(scenario: &str) -> (Task, ReviewResult) {
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().findings.push(FindingRecord {
        id: "F-1".into(),
        text: "P1: the parser drops the last row".into(),
        status: FindingStatus::Open,
        severity: None,
        attempt: 1,
        round: 1,
        repeats: 2,
        scenarios: vec![],
        handled: false,
    });
    let r = ReviewResult {
        verdict: Verdict::Fail,
        findings: vec!["P1: the parser drops the last row [F-1]".into()],
        repeated: vec!["P1: the parser drops the last row [F-1]".into()],
        severities: vec![Some(1)],
        details: vec![FindingDetail {
            id: Some("F-1".into()),
            scenario: Some(FindingScenario {
                run: scenario.into(),
                fails: "red".into(),
            }),
            ..Default::default()
        }],
        criteria: Vec::new(),
    };
    (task, r)
}

#[tokio::test]
async fn a_review_finding_with_a_prose_scenario_gets_no_check() {
    let (app, dir) = test_app();
    let wt = dir.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let (mut task, mut r) = repeated_finding("Fix the parser; touch ran-marker");
    task.fixture_data().worktree = wt.to_string_lossy().into();
    app.store.save_task(&task).unwrap();
    handle_repeated_findings(&app, &mut task, 1, &mut r, &CancelToken::new())
        .await
        .unwrap();
    assert!(!wt.join("ran-marker").exists(), "the scenario was run");
    assert!(task.findings[0].handled);
    assert_eq!(task.criteria, vec!["Review finding F-1 is fixed"]);
    assert!(task.checks.is_empty());
    let lines: Vec<_> = task
        .decisions
        .iter()
        .filter(|d| d.contains("has no check"))
        .collect();
    assert_eq!(lines.len(), 1, "{:?}", task.decisions);
    assert!(lines[0].contains("criterion 0"));
    assert_eq!(task.status, TaskStatus::Running);
    assert!(task.question.is_none());
}

#[tokio::test]
async fn a_review_finding_with_a_failing_command_scenario_still_gets_a_check() {
    let (app, dir) = test_app();
    let wt = dir.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let (mut task, mut r) = repeated_finding("node scripts/a.mjs");
    task.fixture_data().worktree = wt.to_string_lossy().into();
    app.store.save_task(&task).unwrap();
    handle_repeated_findings(&app, &mut task, 1, &mut r, &CancelToken::new())
        .await
        .unwrap();
    assert_eq!(task.criteria.len(), 1);
    assert!(task.criteria[0].contains("-- check: node scripts/a.mjs"));
    assert_eq!(task.checks.len(), 1);
    assert_eq!(task.checks[0].baseline, Some(Baseline::Fail));
}
