use super::*;

#[test]
fn landing_a_done_task_while_its_pull_request_is_drafted_cancels_the_pull_request() {
    let reply = "{\"branch\":\"feature/late\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup_full(&script(reply, true), Pr::Default, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s, "draft-started");
    // The owner turns landing on and lands the task while the draft runs.
    let mut settings = s.daemon.request("settings.get", serde_json::json!({}));
    settings["landOnDefaultRepos"] = serde_json::json!({ s.repo.path().to_str().unwrap(): true });
    s.daemon
        .request("settings.set", serde_json::json!({"settings": settings}));
    s.daemon.request("task.land", serde_json::json!({"id": id}));
    let landed = poll_until(&s.daemon, &id, |st| st == "done" || st == "failed");
    assert!(landed["landedSha"].is_string(), "{landed}");
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    wait_for(&s, "drafts.log");
    stays("the cancelled pull request staying cancelled", || {
        let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
        task.get("prPending").is_none()
            && task.get("prUrl").is_none()
            && !s.gh_log().contains("pr ")
    });
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task.get("prPending").is_none() && task.get("prUrl").is_none(),
        "{task}"
    );
    assert!(
        !task["decisions"].to_string().contains("PR: not opened"),
        "{task}"
    );
    assert!(!s.gh_log().contains("pr "), "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_done_task_whose_base_moved_is_carried_onto_the_new_base_before_the_push() {
    let s = setup_full(&script("{}", false), Pr::Default, false, FAKE_GH, &[]);
    // Origin's main moves on while the local main (the task's base) stays.
    let other = tempfile::tempdir().unwrap();
    let remote = s._remote.path().to_str().unwrap();
    git_out(other.path(), &["clone", "-q", remote, "."]);
    git_out(
        other.path(),
        &["config", "user.email", "other@example.invalid"],
    );
    git_out(other.path(), &["config", "user.name", "other"]);
    std::fs::write(other.path().join("newer.txt"), "newer\n").unwrap();
    git_out(other.path(), &["add", "."]);
    git_out(other.path(), &["commit", "-q", "-m", "newer main"]);
    git_out(other.path(), &["push", "-q", "origin", "main"]);
    let newer = git_out(other.path(), &["rev-parse", "HEAD"]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    let branch = format!("refs/heads/{}", task["branch"].as_str().unwrap());
    let pushed = git_out(s._remote.path(), &["rev-parse", &branch]);
    git_out(
        s._remote.path(),
        &["merge-base", "--is-ancestor", &newer, &pushed],
    );
    let files = git_out(s._remote.path(), &["ls-tree", "-r", "--name-only", &pushed]);
    assert!(
        files.contains("pr.txt") && files.contains("newer.txt"),
        "{files}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn the_carry_worktree_gets_the_repos_setup_before_its_checks_rerun() {
    let s = setup_full(&script("{}", false), Pr::Default, false, FAKE_GH, &[]);
    let extra = setup_and_moved_base(&s, "touch .env-ready");
    let id = s.create(extra);
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_setup_failure_in_the_carry_worktree_is_the_environment_with_one_notice() {
    let s = setup_full(&script("{}", false), Pr::Default, false, FAKE_GH, &[]);
    let extra = setup_and_moved_base(
        &s,
        "case \"$PWD\" in *pr-carry*) echo broken >&2; exit 4;; esac; touch .env-ready",
    );
    let id = s.create(extra);
    done(&s, &id);
    let task = wait_decision(&s, &id, "setup of the carry worktree failed");
    assert!(
        !task["decisions"]
            .to_string()
            .contains("could not be carried"),
        "{task}"
    );
    wait_notice(&s, &id);
    stays("one notice and no pull request", || {
        notices(&s, &id) == 1 && !s.gh_log().contains("pr create")
    });
    assert_eq!(notices(&s, &id), 1);
    assert!(!s.gh_log().contains("pr create"), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}
