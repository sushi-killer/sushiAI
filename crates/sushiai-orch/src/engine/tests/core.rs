#[test]
fn cancel_token_is_visible_synchronously_after_cancel() {
    let cancel = CancelToken::new();
    assert!(!cancel.is_cancelled());
    cancel.cancel();
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn cancel_token_wakes_a_waiter_registered_after_cancel() {
    let cancel = CancelToken::new();
    cancel.cancel();
    // Must resolve immediately, not hang -- this is exactly the "check
    // between every step" use case.
    tokio::time::timeout(Duration::from_millis(200), cancel.cancelled())
        .await
        .expect("cancelled() must resolve immediately once already cancelled");
}

#[test]
fn validate_task_id_rejects_path_traversal_and_non_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    assert!(validate_task_id(&store, "../../etc/passwd").is_err());
    assert!(validate_task_id(&store, "not-a-uuid").is_err());
    assert!(validate_task_id(&store, "").is_err());
    let real_id = uuid::Uuid::new_v4().to_string();
    assert!(validate_task_id(&store, &real_id).is_ok());
}

#[test]
fn simple_hash_is_stable_and_sensitive_to_content() {
    assert_eq!(simple_hash("abc"), simple_hash("abc"));
    assert_ne!(simple_hash("abc"), simple_hash("abd"));
}
