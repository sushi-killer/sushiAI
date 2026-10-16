use super::{load_base_runs, StoredBaseRun, BASE_RUNS_FILE};
use crate::task::attempt::VerifyOutcome;
#[test]
fn interrupted_results_on_disk_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let interrupted = [124, 137, 143]
        .into_iter()
        .map(|code| StoredBaseRun {
            base_sha: "base".to_string(),
            outcome: VerifyOutcome {
                command: format!("exit {code}"),
                code: Some(code),
                ..Default::default()
            },
        })
        .collect::<Vec<_>>();
    std::fs::write(
        data_dir.join(BASE_RUNS_FILE),
        serde_json::to_string(&interrupted).unwrap(),
    )
    .unwrap();
    assert!(load_base_runs(data_dir).is_empty());
}
