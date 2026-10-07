use std::fs;

use serde_json::{json, Value};
use sushiai_agents::codex_hooks::{
    install, is_ours, merge, remove, uninstall, write_text_atomic, HooksFileError,
};

const BIN: &str = "/home/user/.sushiai/bin/sushiai";

/// Shape of a real `~/.codex/hooks.json` with entries from other tools.
fn user_file() -> Value {
    json!({
        "hooks": {
            "SessionStart": [{"hooks": [{"command": "bash '/home/user/.codex/other-tool.sh' session", "timeout": 10, "type": "command"}]}],
            "Stop": [
                {"hooks": [{"command": "/home/user/.local/bin/notifier", "timeout": 60, "type": "command"}]},
                {"matcher": "x", "hooks": [{"command": "echo mine", "type": "command"}]}
            ],
            "Custom": [{"hooks": []}]
        },
        "unrelated": {"keep": true}
    })
}

#[test]
fn merge_into_empty_inputs() {
    for empty in [Value::Null, json!({}), json!({"hooks": {}})] {
        let v = merge(&empty, BIN, 600).unwrap();
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 8);
        let cmd = hooks["PermissionRequest"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, format!("{BIN} hook permission --agent codex"));
        assert_eq!(hooks["Interrupt"][0]["hooks"][0]["timeout"], 2);
    }
}

#[test]
fn merge_is_idempotent_and_keeps_user_entries() {
    let x = user_file();
    let once = merge(&x, BIN, 600).unwrap();
    let twice = merge(&once, BIN, 600).unwrap();
    assert_eq!(once, twice);
    assert_eq!(once["unrelated"], json!({"keep": true}));
    // User groups are untouched and still first.
    assert_eq!(once["hooks"]["Stop"][0], x["hooks"]["Stop"][0]);
    assert_eq!(once["hooks"]["Stop"][1], x["hooks"]["Stop"][1]);
    assert_eq!(once["hooks"]["Stop"].as_array().unwrap().len(), 3);
    assert_eq!(once["hooks"]["Custom"], x["hooks"]["Custom"]);
}

#[test]
fn merge_updates_our_entry_in_place_when_the_path_changes() {
    let once = merge(&user_file(), BIN, 600).unwrap();
    let moved = merge(&once, "/opt/my dir/sushiai", 600).unwrap();
    let stop = moved["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 3);
    assert_eq!(
        stop[2]["hooks"][0]["command"],
        "'/opt/my dir/sushiai' hook stop --agent codex"
    );
    assert_eq!(remove(&moved).unwrap(), remove(&once).unwrap());
}

#[test]
fn merge_collapses_duplicates_of_ours() {
    let mut v = merge(&user_file(), BIN, 600).unwrap();
    let dup = v["hooks"]["Stop"][2].clone();
    v["hooks"]["Stop"].as_array_mut().unwrap().push(dup);
    let fixed = merge(&v, BIN, 600).unwrap();
    assert_eq!(fixed["hooks"]["Stop"].as_array().unwrap().len(), 3);
}

#[test]
fn remove_of_merge_restores_user_entries() {
    let x = user_file();
    assert_eq!(remove(&merge(&x, BIN, 600).unwrap()).unwrap(), x);
    let y = json!({"hooks": {"Stop": [{"hooks": [{"command": "a", "type": "command"}]}]}, "k": 1});
    assert_eq!(remove(&merge(&y, BIN, 600).unwrap()).unwrap(), y);
    assert_eq!(
        remove(&merge(&json!({}), BIN, 600).unwrap()).unwrap(),
        json!({})
    );
}

#[test]
fn remove_is_a_noop_without_our_entries() {
    let x = json!({"hooks": {"Stop": [{"hooks": [{"command": "a", "type": "command"}]}]}});
    assert_eq!(remove(&x).unwrap(), x);
    assert_eq!(remove(&Value::Null).unwrap(), json!({}));
}

#[test]
fn remove_takes_our_handler_out_of_a_shared_group() {
    let x = json!({"hooks": {"Stop": [{"hooks": [
        {"command": "mine", "type": "command"},
        {"command": "/p/sushiai hook stop --agent codex", "type": "command"}
    ]}]}});
    let r = remove(&x).unwrap();
    assert_eq!(
        r["hooks"]["Stop"][0]["hooks"],
        json!([{"command": "mine", "type": "command"}])
    );
}

#[test]
fn marker_matches_quoted_and_plain_paths_only() {
    assert!(is_ours("/a/sushiai hook stop"));
    assert!(is_ours("'/a b/sushiai' hook stop --agent codex"));
    assert!(!is_ours("/a/other-tool hook stop"));
    assert!(!is_ours("echo sushiai"));
}

#[test]
fn malformed_shapes_are_errors_not_clobbered() {
    assert!(matches!(
        merge(&json!([1]), BIN, 600),
        Err(HooksFileError::Shape(_))
    ));
    assert!(matches!(
        merge(&json!({"hooks": []}), BIN, 600),
        Err(HooksFileError::Shape(_))
    ));
    assert!(matches!(
        merge(&json!({"hooks": {"Stop": {}}}), BIN, 600),
        Err(HooksFileError::Shape(_))
    ));
    assert!(matches!(
        merge(&json!({}), "relative/sushiai", 600),
        Err(HooksFileError::Launch(_))
    ));
    assert!(matches!(
        remove(&json!({"hooks": 3})),
        Err(HooksFileError::Shape(_))
    ));
}

#[test]
fn install_backs_up_writes_atomically_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    let original = serde_json::to_string_pretty(&user_file()).unwrap();
    fs::write(&path, &original).unwrap();

    let out = install(&path, BIN, 600, 1111).unwrap();
    assert!(out.changed);
    let backup = out.backup.unwrap();
    assert_eq!(
        backup,
        fs::canonicalize(dir.path())
            .unwrap()
            .join("hooks.json.bak-1111")
    );
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after, merge(&user_file(), BIN, 600).unwrap());

    // Second install: no change, no new backup, no temp file left.
    let again = install(&path, BIN, 600, 2222).unwrap();
    assert!(!again.changed && again.backup.is_none());
    assert!(!dir.path().join("hooks.json.bak-2222").exists());
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn uninstall_restores_user_content_and_missing_file_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    assert!(!uninstall(&path, 1).unwrap().changed);
    assert!(!path.exists());

    fs::write(&path, serde_json::to_string(&user_file()).unwrap()).unwrap();
    install(&path, BIN, 600, 10).unwrap();
    let out = uninstall(&path, 20).unwrap();
    assert!(out.changed);
    assert!(out.backup.unwrap().ends_with("hooks.json.bak-20"));
    let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["hooks"]["Stop"][0], user_file()["hooks"]["Stop"][0]);
    assert!(!fs::read_to_string(&path).unwrap().contains("sushiai hook"));
}

#[test]
fn install_creates_a_missing_file_without_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    let out = install(&path, BIN, 600, 5).unwrap();
    assert!(out.changed && out.backup.is_none());
    assert!(path.exists());
}

#[test]
fn invalid_json_file_is_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    fs::write(&path, "{ not json").unwrap();
    assert!(matches!(
        install(&path, BIN, 600, 1),
        Err(HooksFileError::Json(_))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn an_atomic_write_is_private_from_the_first_byte_and_keeps_an_old_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;

    let fresh = dir.path().join("fresh.json");
    write_text_atomic(&fresh, "secret", 1).unwrap();
    assert_eq!(mode(&fresh), 0o600);

    let old = dir.path().join("old.json");
    fs::write(&old, "old").unwrap();
    fs::set_permissions(&old, fs::Permissions::from_mode(0o640)).unwrap();
    write_text_atomic(&old, "new", 2).unwrap();
    assert_eq!(fs::read_to_string(&old).unwrap(), "new");
    assert_eq!(mode(&old), 0o640);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "no temp left");
}
