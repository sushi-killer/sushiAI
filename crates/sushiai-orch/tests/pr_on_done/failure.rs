use super::*;

#[test]
fn a_transient_gh_failure_is_retried_and_the_pull_request_opens_without_a_notice() {
    let s = setup_full(
        &script("{}", false),
        Pr::Default,
        false,
        GH_FAILING,
        &[FAST_RETRY],
    );
    std::fs::write(s.tools.path().join("fail-first"), "2").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let task = wait_decision(&s, &id, "PR: try 2 failed");
    assert!(
        task["decisions"].to_string().contains("connection reset"),
        "{task}"
    );
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_transient_failure_that_never_clears_stops_after_the_bound_with_one_notice() {
    let s = setup_full(
        &script("{}", false),
        Pr::Default,
        false,
        GH_FAILING,
        &[FAST_RETRY],
    );
    std::fs::write(s.tools.path().join("fail-first"), "100").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened after 5 tries");
    assert_eq!(task["status"], "done", "{task}");
    wait_notice(&s, &id);
    settle();
    assert_eq!(s.gh_log().matches("pr list").count(), 5, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 1);
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_auth_failure_sends_one_notice_does_not_loop_and_task_pr_opens_it_later() {
    let s = setup_full(
        &script("{}", false),
        Pr::Default,
        false,
        GH_FAILING,
        &[FAST_RETRY],
    );
    std::fs::write(s.tools.path().join("auth-broken"), "").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened: ");
    assert_eq!(task["status"], "done", "{task}");
    assert!(
        task["decisions"].to_string().contains("gh auth login"),
        "{task}"
    );
    wait_notice(&s, &id);
    settle();
    assert_eq!(s.gh_log().matches("pr list").count(), 1, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 1);
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(task.get("prUrl").is_none() && task.get("prPending").is_none());
    // Logged in again: the owner opens it later.
    std::fs::remove_file(s.tools.path().join("auth-broken")).unwrap();
    let opened = s.daemon.request("task.pr", serde_json::json!({"id": id}));
    assert_eq!(opened["prUrl"], URL, "{opened}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_base_checkout_that_stays_dirty_leaves_the_work_on_its_branch_and_opens_the_pull_request() {
    let s = setup_full(
        &script("{}", false),
        Pr::Default,
        true,
        FAKE_GH,
        &[("ORCHD_LANDING_DIRTY_LIMIT_MS", "4000")],
    );
    std::fs::write(s.repo.path().join("README.md"), "hello\nmy edit\n").unwrap();
    let id = s.create(serde_json::json!({}));
    let waiting = poll_until(&s.daemon, &id, |st| {
        matches!(st, "landing" | "done" | "failed" | "stopped")
    });
    assert_eq!(waiting["status"], "landing", "{waiting}");
    // A retry before the bound changes nothing: the retry is its own loop, so
    // wait for its decision to be the only state we can see.
    s.daemon
        .request("task.start", serde_json::json!({"id": id}));
    std::thread::sleep(Duration::from_millis(1000));
    let early = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(early["status"], "landing", "{early}");
    assert!(
        !early["decisions"].to_string().contains("Land: gave up"),
        "{early}"
    );
    std::thread::sleep(Duration::from_millis(3500));
    s.daemon
        .request("task.start", serde_json::json!({"id": id}));
    let task = done(&s, &id);
    assert!(task.get("landedSha").is_none(), "{task}");
    assert!(
        task["decisions"].to_string().contains("Land: gave up"),
        "{task}"
    );
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    assert_eq!(
        std::fs::read_to_string(s.repo.path().join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );
    wait_notice(&s, &id);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_dirty_give_up_does_not_promise_a_pull_request_when_they_are_off() {
    let s = setup_full(
        &script("{}", false),
        Pr::Off,
        true,
        FAKE_GH,
        &[("ORCHD_LANDING_DIRTY_LIMIT_MS", "300")],
    );
    std::fs::write(s.repo.path().join("README.md"), "hello\nmy edit\n").unwrap();
    let id = s.create(serde_json::json!({}));
    poll_until(&s.daemon, &id, |st| st == "landing");
    std::thread::sleep(Duration::from_millis(600));
    s.daemon
        .request("task.start", serde_json::json!({"id": id}));
    done(&s, &id);
    wait_notice(&s, &id);
    let texts = note_texts(&s, &id);
    assert!(texts.iter().all(|t| !t.contains("is opened")), "{texts:?}");
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_repo_without_an_origin_is_noted_once_with_no_draft_and_no_notice() {
    let s = setup_full(&script("{}", false), Pr::Default, false, FAKE_GH, &[]);
    git_out(s.repo.path(), &["remote", "remove", "origin"]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "no origin remote");
    assert_eq!(task["status"], "done", "{task}");
    settle();
    assert!(
        !s.tools.path().join("drafts.log").exists(),
        "a draft ran without an origin"
    );
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_remote_that_is_no_github_host_gets_no_draft_and_no_push() {
    let gh = GH_FAILING.replace(
        "case \"$1 $2\" in",
        "case \"$1 $2\" in\n  \"repo view\") echo \"none of the git remotes configured for this repository point to a known GitHub host\" >&2; exit 1 ;;",
    );
    let s = setup_full(&script("{}", false), Pr::Default, false, &gh, &[FAST_RETRY]);
    let first = s.create(serde_json::json!({}));
    done(&s, &first);
    wait_decision(&s, &first, "known GitHub host");
    let second = s.create(serde_json::json!({"title": "Second"}));
    done(&s, &second);
    wait_decision(&s, &second, "known GitHub host");
    settle();
    assert!(
        !s.tools.path().join("drafts.log").exists(),
        "a draft ran for a non-GitHub remote"
    );
    assert_eq!(s.gh_log().matches("pr list").count(), 0, "{}", s.gh_log());
    let heads = git_out(s._remote.path(), &["for-each-ref", "refs/heads"]);
    assert_eq!(heads.lines().count(), 1, "pushed: {heads}");
    assert_eq!(notices(&s, &first), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn the_overseer_cannot_open_a_pull_request_orchd_owns() {
    let reply = "{\"branch\":\"feature/own\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup_full(&script(reply, true), Pr::Default, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s.tools.path().join("draft-started"));
    let err = s.daemon.request_error(
        "task.pr",
        serde_json::json!({"id": id, "source": "overseer"}),
    );
    assert!(err.contains("orchd opens this task"), "{err}");
    assert!(!s.gh_log().contains("pr "), "{}", s.gh_log());
    // The owner may still open it.
    let opened = s.daemon.request("task.pr", serde_json::json!({"id": id}));
    assert_eq!(opened["prUrl"], URL, "{opened}");
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_workflow_scope_rejection_is_permanent_with_one_try_and_one_note() {
    let s = setup_full(
        &script("{}", false),
        Pr::Default,
        false,
        FAKE_GH,
        &[FAST_RETRY],
    );
    let hook = s._remote.path().join("hooks/pre-receive");
    let log = s._remote.path().join("rejects.log");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\necho x >> '{}'\necho \"error: refusing to allow a Personal Access Token to create or update workflow \\`.github/workflows/ci.yml\\` without \\`workflow\\` scope\" >&2\nexit 1\n",
            log.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened: ");
    assert!(
        task["decisions"].to_string().contains("`workflow` scope"),
        "{task}"
    );
    wait_notice(&s, &id);
    settle();
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    assert_eq!(notices(&s, &id), 1);
    assert!(
        note_texts(&s, &id)[0].contains("workflow"),
        "{:?}",
        note_texts(&s, &id)
    );
    s.daemon.shutdown_and_wait();
}
