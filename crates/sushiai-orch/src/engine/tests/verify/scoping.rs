fn desktop_scope() -> Vec<ScopedCheck> {
    vec![ScopedCheck {
        repo: None,
        command: "npm run test:desktop".into(),
        paths: [
            "src/app/**",
            "src/extensions/**",
            "electron/**",
            "src/styles/**",
            "*.html",
        ]
        .map(String::from)
        .to_vec(),
    }]
}

#[test]
fn a_scoped_command_is_skipped_when_no_changed_file_is_under_its_paths() {
    let commands = vec!["npm run test:desktop".to_string(), "npm test".to_string()];
    let (run, lines) = filter_scoped(
        &desktop_scope(),
        "/repo",
        &commands,
        &["orchd/src/x.rs".into()],
    );
    assert_eq!(run, vec!["npm test"]);
    assert_eq!(
        lines,
        vec![
            "Orchestrator: skipped npm run test:desktop: no change under src/app/**, src/extensions/**, electron/**, src/styles/**, *.html"
        ]
    );
}

#[test]
fn a_scoped_command_runs_when_a_changed_file_matches() {
    let commands = vec!["npm run test:desktop".to_string()];
    for file in [
        "electron/main.cjs",
        "src/app/Shell.tsx",
        "index.html",
        "web/x.html",
    ] {
        let (run, lines) = filter_scoped(&desktop_scope(), "/repo", &commands, &[file.into()]);
        assert_eq!(run, commands, "{file}");
        assert!(lines.is_empty(), "{file}");
    }
}

#[test]
fn a_scoped_entry_for_another_repo_or_without_paths_does_not_scope() {
    let commands = vec!["make slow".to_string()];
    let other = vec![ScopedCheck {
        repo: Some("/other".into()),
        command: "make *".into(),
        paths: vec!["lib/**".into()],
    }];
    assert_eq!(filter_scoped(&other, "/repo", &commands, &[]).0, commands);
    let empty = vec![ScopedCheck {
        repo: None,
        command: "make *".into(),
        paths: vec![],
    }];
    assert_eq!(filter_scoped(&empty, "/repo", &commands, &[]).0, commands);
    let here = vec![ScopedCheck {
        repo: Some("/repo".into()),
        command: "make *".into(),
        paths: vec!["lib/**".into()],
    }];
    assert!(filter_scoped(&here, "/repo", &commands, &["app/x".into()])
        .0
        .is_empty());
}

#[test]
fn the_base_preflight_scopes_by_planned_paths() {
    let commands = vec!["npm run test:desktop".to_string(), "npm test".to_string()];
    let scope = desktop_scope();
    let run = |planned: &[&str]| {
        let planned: Vec<String> = planned.iter().map(|p| p.to_string()).collect();
        filter_scoped_planned(&scope, "/repo", &commands, &planned)
    };
    assert_eq!(run(&["orchd/src/model.rs", "docs/x.md"]), vec!["npm test"]);
    assert_eq!(run(&["electron/main.cjs"]), commands);
    // A planned directory counts when a glob falls under it.
    assert_eq!(run(&["src/app"]), commands);
    assert_eq!(run(&["src/"]), commands);
    assert_eq!(run(&["src/agents"]), vec!["npm test"]);
    assert_eq!(run(&["docs"]), vec!["npm test"]);
    assert_eq!(run(&["page.html"]), commands);
    // No planned paths: no way to tell, so it runs.
    assert_eq!(run(&[]), commands);
}
