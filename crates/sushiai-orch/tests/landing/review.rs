use super::*;

#[test]
fn a_landing_carry_that_rewrites_a_reviewed_file_runs_verify_and_the_review_again() {
    let gates = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", REREVIEW_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["briefCheckRoute"] = serde_json::json!("claude-sonnet");
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = repo_on_work_branch();
    let root = repo.path();
    let lines: String = (1..=9).map(|i| format!("l{i}\n")).collect();
    std::fs::write(root.join("shared.txt"), lines).unwrap();
    git_out(root, &["add", "shared.txt"]);
    git_out(root, &["commit", "-q", "-m", "shared lines"]);

    // y's verify waits on a gate and counts its runs; on the base it fails.
    let g = gates.path().display();
    let y_verify = format!(
        "case \"$PWD\" in */base-runs/*) exit 1;; esac; echo run >> {g}/verify.log; while [ -e {g}/hold_verify ]; do sleep 0.1; done; grep -q Y9 shared.txt"
    );
    std::fs::write(gates.path().join("hold_verify"), "").unwrap();
    let y = create(&daemon, root, "y", &y_verify, true);
    let x = create(&daemon, root, "x", "grep -q X1 shared.txt", true);
    let (x_id, y_id) = (
        x["id"].as_str().unwrap().to_string(),
        y["id"].as_str().unwrap().to_string(),
    );
    let lines_of = |name: &str| {
        std::fs::read_to_string(gates.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    daemon.request("task.start", serde_json::json!({"id": y_id}));
    wait_until("y's verify to run on its own tree", || {
        (lines_of("verify.log").len() == 1).then_some(())
    });
    // x lands while y's verify holds: the branch moves under y's review.
    daemon.request("task.start", serde_json::json!({"id": x_id}));
    let x_done = until_done(&daemon, &x_id);
    assert_eq!(x_done["status"], "done", "{x_done}");
    std::fs::remove_file(gates.path().join("hold_verify")).unwrap();
    let y_done = until_done(&daemon, &y_id);
    assert_eq!(y_done["status"], "done", "{y_done}");

    // One implement attempt, no conflict: the carry merged cleanly but
    // rewrote shared.txt, so verify and the review ran on the carried tree.
    let implements = y_done["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count();
    assert_eq!(implements, 1, "{y_done}");
    assert!(
        decisions(&y_done).contains("Land: the carry onto work changed shared.txt"),
        "{y_done}"
    );
    assert_eq!(
        lines_of("verify.log").len(),
        2,
        "{:?}",
        lines_of("verify.log")
    );
    let reviews: Vec<String> = lines_of("reviews")
        .into_iter()
        .filter(|n| n == "y")
        .collect();
    assert_eq!(reviews.len(), 2, "{reviews:?}");
    let shared = std::fs::read_to_string(root.join("shared.txt")).unwrap();
    assert!(shared.contains("X1") && shared.contains("Y9"), "{shared}");
    assert_eq!(y_done["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}

#[test]
fn a_rereview_after_the_landing_waited_reviews_the_same_attempt_again() {
    let gates = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", REREVIEW_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["briefCheckRoute"] = serde_json::json!("claude-sonnet");
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = repo_on_work_branch();
    let root = repo.path();
    let lines: String = (1..=9).map(|i| format!("l{i}\n")).collect();
    std::fs::write(root.join("shared.txt"), lines).unwrap();
    git_out(root, &["add", "shared.txt"]);
    git_out(root, &["commit", "-q", "-m", "shared lines"]);

    let g = gates.path().display();
    let verify = format!(
        "case \"$PWD\" in */base-runs/*) exit 1;; esac; echo run >> {g}/verify.log; grep -q Y9 shared.txt"
    );
    // A dirty base checkout makes the passed task wait in landing.
    std::fs::write(root.join("README.md"), "hello\nmy edit\n").unwrap();
    let y = create(&daemon, root, "y", &verify, true);
    let id = y["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let waiting = until_dirty_landing(&daemon, &id);
    assert_eq!(waiting["status"], "landing", "{waiting}");

    // The branch moves in a file the review saw, then the checkout is clean.
    let shared = std::fs::read_to_string(root.join("shared.txt")).unwrap();
    std::fs::write(root.join("shared.txt"), shared.replace("l1\n", "X1\n")).unwrap();
    git_out(root, &["add", "shared.txt"]);
    git_out(
        root,
        &["commit", "-q", "-m", "owner edit", "--", "shared.txt"],
    );
    git_out(root, &["checkout", "--", "README.md"]);
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");

    let attempts = done["attempts"].as_array().unwrap();
    let implements = attempts.iter().filter(|a| a["stage"] == "implement");
    assert_eq!(implements.count(), 1, "{done}");
    assert!(attempts.iter().all(|a| a["status"] != "running"), "{done}");
    let count = |name: &str| {
        std::fs::read_to_string(gates.path().join(name))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty() && (name == "verify.log" || *l == "y"))
            .count()
    };
    assert_eq!(count("verify.log"), 2);
    assert_eq!(count("reviews"), 2);
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}
