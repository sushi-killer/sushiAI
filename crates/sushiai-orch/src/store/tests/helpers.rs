use super::super::Store;

#[test]
fn helpers_folder_claims_are_unique_and_read_in_numeric_sequence_order() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path()).unwrap();
    let first = store
        .claim_helper_run_dir("task", "review", Some(1))
        .unwrap();
    std::fs::write(first.join("events.jsonl"), "first").unwrap();
    let mut claimed = vec![first.clone()];
    for _ in 0..10 {
        claimed.push(
            store
                .claim_helper_run_dir("task", "review", Some(1))
                .unwrap(),
        );
    }
    store
        .claim_helper_run_dir("task", "review", Some(2))
        .unwrap();
    let detached = store
        .claim_helper_run_dir("task", "brief-check", None)
        .unwrap();
    assert_eq!(store.helper_run_dirs("task", "review", Some(1)), claimed);
    assert_eq!(first, tmp.path().join("tasks/task/helpers/review-1"));
    assert_eq!(
        std::fs::read_to_string(first.join("events.jsonl")).unwrap(),
        "first"
    );
    assert_eq!(
        store
            .helper_run_dirs("task", "brief-check", None)
            .as_slice(),
        std::slice::from_ref(&detached)
    );
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(detached.join("helper.json")).unwrap())
            .unwrap();
    assert_eq!(
        meta,
        serde_json::json!({"stage":"brief-check", "attempt":null})
    );
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let store = &store;
            scope.spawn(move || {
                store
                    .claim_helper_run_dir("task", "advisor", Some(1))
                    .unwrap();
            });
        }
    });
    assert_eq!(store.helper_run_dirs("task", "advisor", Some(1)).len(), 4);
}
