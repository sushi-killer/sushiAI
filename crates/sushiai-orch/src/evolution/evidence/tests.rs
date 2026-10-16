use super::*;
use crate::engine::evolution::test_support::*;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::evolution::signals::{ExcerptRef, SignalKind};
use crate::task::TaskStatus;

#[test]
fn evolution_live_signals_drop_deleted_and_archived_tasks() {
    let (app, _dir) = test_app();
    let live = task_with_status(TaskStatus::Done);
    let mut archived = task_with_status(TaskStatus::Done);
    archived.fixture_data().archived = true;
    app.store.save_task(&live).unwrap();
    app.store.save_task(&archived).unwrap();
    let dir = evolution_dir(&app.data_dir);
    for id in [live.id.as_str(), archived.id.as_str(), "deleted-task"] {
        let s = waste(signal(SignalKind::Loop, id, "/r", "x"), 1, 0.0).signal;
        append_line(
            &dir.join("signals.jsonl"),
            &serde_json::to_string(&s).unwrap(),
        )
        .unwrap();
    }
    let ids: Vec<String> = read_live_signals(&app.data_dir)
        .into_iter()
        .map(|s| s.signal.task_id)
        .collect();
    assert_eq!(ids, vec![live.id.clone()]);
    assert_eq!(read_signals(&app.data_dir).len(), 3);
}

#[test]
fn evolution_excerpt_must_resolve_under_the_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(data.join("runs")).unwrap();
    let body: String = (1..=100).map(|n| format!("line {n}\n")).collect();
    std::fs::write(data.join("runs/events.jsonl"), body).unwrap();
    std::fs::write(dir.path().join("secret.txt"), "secret\n").unwrap();
    let mut s = waste(signal(SignalKind::Loop, "a", "/r", "d"), 1, 0.0);
    s.signal.excerpt_ref = ExcerptRef {
        file: "runs/events.jsonl".into(),
        from_line: 50,
        to_line: 52,
    };
    let e = read_excerpt(&data, &s.signal).unwrap();
    assert_eq!(e.lines.len(), 40);
    assert!(e.lines.contains(&"line 50".to_string()));
    assert!(e.lines.contains(&"line 52".to_string()));
    s.signal.excerpt_ref.file = "../secret.txt".into();
    assert!(read_excerpt(&data, &s.signal).is_none());
    s.signal.excerpt_ref.file = dir.path().join("secret.txt").to_string_lossy().to_string();
    assert!(read_excerpt(&data, &s.signal).is_none());
}
