use super::where_to_look_block;

#[test]
fn where_to_look_excerpts_files_and_stays_under_the_cap() {
    let dir = std::env::temp_dir().join(format!("orchd-wtl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/sub")).unwrap();
    let big: String = (0..500)
        .map(|i| format!("line {i} {}\n", "x".repeat(60)))
        .collect();
    let mut paths = Vec::new();
    for i in 0..6 {
        std::fs::write(dir.join(format!("src/f{i}.rs")), &big).unwrap();
        paths.push(format!("src/f{i}.rs"));
    }
    std::fs::write(dir.join("src/b.txt"), "alpha\n").unwrap();
    paths.push("src/sub".to_string());
    let block = where_to_look_block(&dir, &paths);
    assert!(block.starts_with("## Where to look"), "{block}");
    assert!(block.contains("line 0 "), "{block}");
    assert!(!block.contains("line 40 "), "{block}");
    assert!(block.matches("<untrusted-data>").count() >= 1);
    for chunk in block.split("<untrusted-data>\n").skip(1) {
        let body = chunk.split("</untrusted-data>").next().unwrap();
        assert!(body.lines().count() <= 40, "{body}");
    }
    assert!(block.len() <= 8 * 1024, "{}", block.len());
    // The cap stops later entries instead of cutting one.
    assert!(!block.contains("src/sub/"), "{block}");

    let listed = where_to_look_block(&dir, &["src".to_string()]);
    assert!(listed.contains("`src/`: "), "{listed}");
    assert!(
        listed.contains("b.txt, f0.rs") && listed.contains("sub/"),
        "{listed}"
    );
    assert!(!listed.contains("<untrusted-data>"), "{listed}");

    assert_eq!(where_to_look_block(&dir, &[]), "");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn where_to_look_skips_paths_outside_the_worktree() {
    let base = std::env::temp_dir().join(format!("orchd-wtl-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join("wt");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(base.join("secret.txt"), "outside-secret").unwrap();
    std::fs::write(dir.join("ok.txt"), "inside-ok").unwrap();
    std::fs::write(dir.join("bin.dat"), [b'a', 0, b'b']).unwrap();
    std::fs::write(dir.join("bad.txt"), [0xff, 0xfe, b'x']).unwrap();
    std::os::unix::fs::symlink(base.join("secret.txt"), dir.join("link.txt")).unwrap();
    let paths: Vec<String> = [
        &base.join("secret.txt").display().to_string(),
        "../secret.txt",
        "link.txt",
        "missing.txt",
        "bin.dat",
        "bad.txt",
        "ok.txt",
    ]
    .iter()
    .map(|p| p.to_string())
    .collect();
    let block = where_to_look_block(&dir, &paths);
    assert!(block.contains("inside-ok"), "{block}");
    for bad in [
        "outside-secret",
        "secret.txt",
        "link.txt",
        "missing",
        "bin.dat",
        "bad.txt",
    ] {
        assert!(!block.contains(bad), "{bad} in {block}");
    }
    assert_eq!(where_to_look_block(&dir, &paths[..6]), "");
    std::fs::remove_dir_all(&base).unwrap();
}
