use super::*;

#[test]
fn project_mcp_selection_excludes_worktree_and_unenabled_connected_tools() {
    let env = spawn();
    let repo_root = git_out(env.repo.path(), &["rev-parse", "--show-toplevel"]);
    let repo = repo_root.trim();
    env.daemon
        .request("secrets.set", json!({"projectRepos": {repo: "project-1"}}));
    let mut settings = env.daemon.request("settings.get", json!({}));
    settings["chatTools"].as_array_mut().unwrap().push(json!({
        "id": "global", "label": "Global tool", "enabled": true,
        "server": {"ref": "claude-json:fake"},
    }));
    settings["mcpEnable"] = json!({repo: ["fake", "unknown-enable"]});
    settings["mcpDisable"] = json!({repo: ["disabled", "sushiai-messages", "unknown-disable"]});
    env.daemon
        .request("settings.set", json!({"settings": settings}));
    env.daemon.request("chat.tools", json!({"refresh": true}));
    env.gate("hold", "");
    let id = env.create(&[]);
    assert_eq!(env.get(&id)["repo"], repo);
    std::fs::write(
        env.daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("mcp.json"),
        json!({"mcpServers": {
            "lookup": {"command": "true", "args": []},
            "disabled": {"command": "true", "codex": true},
            "sushiai-messages": {"command": "should-be-replaced"},
        }})
        .to_string(),
    )
    .unwrap();
    env.start(&id);
    env.wait_run(1);

    let mcp: serde_json::Value = serde_json::from_str(&env.gate_file("mcp_1")).unwrap();
    let servers: Vec<&str> = mcp["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(servers, vec!["fake", "lookup", "sushiai-messages"]);
    assert_ne!(
        mcp["mcpServers"]["sushiai-messages"]["command"],
        "should-be-replaced"
    );
    let allow = allow_list(&env, 1);
    // Only a declared read-only hint makes a read: the name `find_item` does not.
    for read in ["mcp__fake__get_item", "mcp__fake__post_quiet"] {
        assert!(allow.contains(&read.to_string()), "{read} in {allow:?}");
    }
    assert!(!allow.iter().any(|a| a.contains("send_note")), "{allow:?}");
    assert!(!allow.iter().any(|a| a.contains("find_item")), "{allow:?}");
    let brief = env.gate_file("brief_1");
    assert!(brief.contains("fake: read tools"), "{brief}");
    assert!(
        brief.contains("global (Global tool): connected but off for this repo"),
        "{brief}"
    );
    assert!(
        brief.contains("asana (Asana): connected but off for this repo"),
        "{brief}"
    );
    assert!(brief.contains("No tool: "), "{brief}");
    for (key, name) in [
        ("mcpEnable", "unknown-enable"),
        ("mcpDisable", "unknown-disable"),
    ] {
        let line = format!("MCP: ignored unknown name {name:?} in {key} for {repo}");
        assert_eq!(
            env.get(&id)["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d.as_str() == Some(line.as_str()))
                .count(),
            1
        );
    }
    env.ungate("hold");
    env.until(&id, "done");
    env.daemon.shutdown_and_wait();
}

#[test]
fn a_web_call_is_decided_by_the_allowed_domains_and_never_held_for_the_owner() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    env.start(&id);
    env.wait_run(1);
    let token = env.token(1);
    let (decision, _) = verdict(&env.hook(&token, "WebFetch", json!({"url": "https://docs.rs/x"})));
    assert_eq!(decision, "allow");

    let mut settings = env.daemon.request("settings.get", json!({}));
    settings["allowedDomains"] = json!(["docs.rs"]);
    env.daemon
        .request("settings.set", json!({"settings": settings}));
    let (decision, _) = verdict(&env.hook(&token, "WebFetch", json!({"url": "https://docs.rs/x"})));
    assert_eq!(decision, "allow");
    let (decision, reason) =
        verdict(&env.hook(&token, "WebFetch", json!({"url": "https://other.test/x"})));
    assert_eq!(decision, "deny");
    assert!(reason.contains("allowed domains"), "{reason}");
    assert_eq!(env.get(&id)["status"], "running", "no question was asked");
    env.ungate("hold");
    env.until(&id, "done");
    env.daemon.shutdown_and_wait();
}

#[test]
fn an_edit_in_the_own_worktree_is_allowed_and_one_that_climbs_out_is_not() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    env.start(&id);
    env.wait_run(1);
    let token = env.token(1);
    let worktree = env.get(&id)["worktree"].as_str().unwrap().to_string();

    for file in [
        format!("{worktree}/crates/x/tests/new.rs"),
        format!("{worktree}/src/lib.rs"),
    ] {
        let result = env.hook(&token, "Edit", json!({"file_path": file}));
        assert_eq!(verdict(&result).0, "allow", "{file}: {result}");
    }
    let mut outside = vec![
        format!("{worktree}/../outside.rs"),
        "../outside.rs".to_string(),
        "~/.zshrc".to_string(),
        "src/lib.rs".to_string(),
    ];
    #[cfg(unix)]
    {
        let wt = Path::new(&worktree);
        std::os::unix::fs::symlink(env.repo.path(), wt.join("out-link")).unwrap();
        std::os::unix::fs::symlink(env.repo.path().join("not-yet.txt"), wt.join("dangling"))
            .unwrap();
        outside.push(format!("{worktree}/out-link/escaped.rs"));
        outside.push(format!("{worktree}/dangling"));
    }
    for file in outside {
        let result = env.hook(&token, "Edit", json!({"file_path": file}));
        assert_eq!(verdict(&result).0, "", "{file}: {result}");
    }
    env.ungate("hold");
    env.until(&id, "done");
    env.daemon.shutdown_and_wait();
}

#[test]
fn an_allowed_write_under_claude_goes_through_staging_and_lands_before_verify() {
    let env = spawn();
    env.gate("hold", "");
    let id = env.create(&[]);
    env.start(&id);
    env.wait_run(1);
    let token = env.token(1);
    let worktree = env.get(&id)["worktree"].as_str().unwrap().to_string();
    let target = format!("{worktree}/.claude/skills/x/SKILL.md");

    let (result, waiting) = env.held(
        &token,
        &id,
        ("Write", json!({"file_path": target, "content": "hi"})),
        "Allow once",
    );
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Allow .claude/skills/x/SKILL.md for this task?"));
    let (decision, reason) = verdict(&result);
    // A hook `allow` does not pass Claude Code's protected-path check, so the
    // agent is sent to the staging dir instead.
    assert_eq!(decision, "deny");
    assert!(
        reason.contains(".orchd-staging/.claude/skills/x/SKILL.md"),
        "{reason}"
    );

    // The agent writes the staged copy; an unapproved staged file is ignored.
    let staged = Path::new(&worktree).join(".orchd-staging/.claude/skills/x/SKILL.md");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, "staged body").unwrap();
    let stray = Path::new(&worktree).join(".orchd-staging/.claude/other.md");
    std::fs::write(&stray, "never approved").unwrap();

    env.ungate("hold");
    let done = env.until(&id, "done");
    // What was committed: the staged body at the real path, nothing else.
    let branch = done["branch"].as_str().unwrap();
    let repo = env.repo.path();
    assert_eq!(
        git_out(
            repo,
            &["show", &format!("{branch}:.claude/skills/x/SKILL.md")]
        )
        .trim(),
        "staged body"
    );
    let files = git_out(repo, &["ls-tree", "-r", "--name-only", branch]);
    assert!(!files.contains(".claude/other.md"), "{files}");
    assert!(!files.contains(".orchd-staging"), "{files}");
    env.daemon.shutdown_and_wait();
}
