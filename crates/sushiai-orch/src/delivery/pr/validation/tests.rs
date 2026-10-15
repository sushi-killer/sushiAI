use super::*;
use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, TaskStatus, VerifyOutcome};

#[test]
fn recorded_checks_replace_drafter_disclaimers_and_keep_exact_tree_provenance() {
    let mut task = sample_task("validation", TaskStatus::Done, AttemptStatus::Passed);
    task.fixture_data().attempts[0].verify = vec![
        VerifyOutcome {
            command: "npm run ci".into(),
            code: Some(0),
            tree: Some("tree-ci".into()),
            commit: Some("commit-checkout".into()),
            ..Default::default()
        },
        VerifyOutcome {
            command: "cargo test".into(),
            code: Some(101),
            tree: Some("tree-uncommitted".into()),
            ..Default::default()
        },
        VerifyOutcome {
            command: "legacy-check".into(),
            code: None,
            ..Default::default()
        },
    ];
    let commits = BTreeMap::from([("tree-ci".into(), "commit-ci".into())]);
    let text = section(&task, &commits);
    assert!(text.contains(
        "`npm run ci`: passed; checkout commit `commit-checkout`; checked tree `tree-ci`"
    ));
    assert!(text
        .contains("`cargo test`: failed (exit 101); checked tree `tree-uncommitted` (checkout commit not recorded)"));
    assert!(text.contains(
        "`legacy-check`: interrupted or no exit code; checkout commit and tree not recorded"
    ));
    let drafted = "## Changes\nAdds saving.\n\n## Validation\nI did not re-run these checks.\nI did not run npm run ci.\n\n## Why\nKeep settings.\n";
    let cleaned = without_section(drafted);
    assert!(cleaned.contains("Adds saving.") && cleaned.contains("Keep settings."));
    assert!(!cleaned.contains("did not") && !cleaned.contains(HEADING));
}
