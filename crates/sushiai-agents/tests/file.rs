//! The crate's one atomic writer: the mode it leaves on the file, the temp file
//! it never leaves behind, and the parent directory it creates when asked.

use std::fs;

use sushiai_agents::file::write_atomic;

#[test]
fn an_atomic_write_is_private_from_the_first_byte_and_keeps_an_old_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;

    let fresh = dir.path().join("fresh.json");
    write_atomic(&fresh, b"secret", false).unwrap();
    assert_eq!(mode(&fresh), 0o600);

    let old = dir.path().join("old.json");
    fs::write(&old, "old").unwrap();
    fs::set_permissions(&old, fs::Permissions::from_mode(0o640)).unwrap();
    write_atomic(&old, b"new", false).unwrap();
    assert_eq!(fs::read_to_string(&old).unwrap(), "new");
    assert_eq!(mode(&old), 0o640);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "no temp left");
}

#[test]
fn the_parent_directory_is_created_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nested").join("deep").join("SKILL.md");

    assert!(write_atomic(&file, b"x", false).is_err());
    assert!(!file.exists());

    write_atomic(&file, b"x", true).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn a_path_without_a_file_name_is_an_error() {
    assert!(write_atomic(std::path::Path::new(".."), b"x", false).is_err());
}
