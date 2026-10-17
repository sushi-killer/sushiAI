use super::*;

#[test]
fn a_parent_landing_conflict_gets_one_agent_resolution_and_lands_both_intents() {
    let gates = tempfile::tempdir().unwrap();
    configure(
        gates.path(),
        "parent",
        "resolve_conflict",
        "shared.txt",
        false,
    );
    configure(gates.path(), "part", "resolve_conflict", "shared.txt", true);
    let (daemon, _scripts) = daemon(gates.path());
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = repo();
    let initial = git_out(repo.path(), &["rev-parse", "HEAD"]);
    let parent = create(
        &daemon,
        repo.path(),
        "parent",
        json!({"paths":[],"verify":["test \"$(cat health.txt)\" = ok"],"variant":{"land":true}}),
    );
    let part = create(
        &daemon,
        repo.path(),
        "part",
        json!({"paths":["shared.txt"],"parent":parent,"verify":["true"]}),
    );
    started(&daemon, gates.path(), "part", 1);
    assert_eq!(get(&daemon, &parent)["baseSha"], initial);
    assert_eq!(
        std::fs::read_to_string(gates.path().join("head_part_1")).unwrap(),
        initial
    );
    commit_file(repo.path(), "shared.txt", "base change\n");
    std::fs::remove_file(gates.path().join("hold_part_1")).unwrap();
    let done = settle(&daemon, &parent);
    let child = get(&daemon, &part);
    capture_parent_evidence(gates.path(), repo.path(), &done, &child);
    for task in [&child, &done] {
        assert_eq!(task["status"], "done", "{task}");
        assert!(
            task["landedAt"].is_number() && task["question"].is_null(),
            "{task}"
        );
        assert_eq!(
            task["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|attempt| attempt["stage"] == "implement")
                .count(),
            1,
            "{task}"
        );
        assert!(task["prUrl"].is_null(), "{task}");
    }
    assert_eq!(child["baseRef"], done["branch"]);
    let brief = std::fs::read_to_string(gates.path().join("brief_parent_1")).unwrap();
    assert!(
        brief.contains("## Base moved") && brief.contains("shared.txt"),
        "{brief}"
    );
    assert!(!brief.contains("## Finishing this task"), "{brief}");
    let markers = std::fs::read_to_string(gates.path().join("conflict_markers")).unwrap();
    assert!(
        markers.contains("<<<<<<<")
            && markers.contains("base change")
            && markers.contains("part change"),
        "{markers}"
    );
    assert_eq!(
        git_out(
            repo.path(),
            &[
                "show",
                &format!("{}:shared.txt", done["landedSha"].as_str().unwrap())
            ],
        ),
        "base change\npart change"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("shared.txt")).unwrap(),
        "base change\npart change\n"
    );
    assert_eq!(
        git_out(repo.path(), &["rev-parse", "work"]),
        done["landedSha"]
    );
    assert!(git_out(repo.path(), &["status", "--porcelain"]).is_empty());
    for name in ["parent", "part"] {
        assert_eq!(
            std::fs::read_to_string(gates.path().join(format!("count_{name}"))).unwrap(),
            "1"
        );
    }
    assert!(!gates.path().join("pr_helper_called").exists());
    daemon.shutdown_and_wait();
}

fn capture_parent_evidence(gates: &Path, repo: &Path, parent: &Value, child: &Value) {
    let Some(directory) = std::env::var_os("SUSHIAI_TEST_PARENT_EVIDENCE") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    for entry in std::fs::read_dir(gates).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
        }
    }
    for (name, task) in [("parent", parent), ("child", child)] {
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec_pretty(task).unwrap(),
        )
        .unwrap();
    }
    std::fs::write(
        directory.join("history.txt"),
        git_out(repo, &["log", "--all", "--graph", "--oneline", "-20"]),
    )
    .unwrap();
}
