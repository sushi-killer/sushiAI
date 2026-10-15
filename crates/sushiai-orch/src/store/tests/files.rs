use super::super::files::*;
use std::fs;

#[cfg(unix)]
#[test]
fn write_json_private_is_mode_0600_and_leaves_no_temp_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    write_json_private(&path, &serde_json::json!({"token": "t"})).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_secret_writes_of_one_path_do_not_collide() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                (0..50).all(|_| write_secret_file(&path, &format!("k{i}")).is_ok())
            })
        })
        .collect();
    for t in threads {
        assert!(t.join().unwrap(), "a write failed on a shared temp name");
    }
}

#[test]
fn write_secret_file_is_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    write_secret_file(&path, "abc123\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "abc123\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
