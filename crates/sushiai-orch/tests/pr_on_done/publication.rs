use super::*;

#[test]
fn a_done_task_refused_on_the_default_branch_opens_the_drafted_pull_request() {
    let reply = "{\"branch\":\"feature/drafted-name\",\"title\":\"feat: drafted title\",\"body\":\"Drafted body for the owner\"}";
    let s = setup(reply, true);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    // The pull request head is the conventional name the draft chose.
    let log = s.gh_log();
    assert!(
        log.contains("pr create --base main --head feature/drafted-name --title feat: drafted title --body Drafted body for the owner"),
        "{log}"
    );
    let heads = git_out(
        s._remote.path(),
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    );
    assert!(heads.contains("feature/drafted-name"), "{heads}");
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(task["status"], "done");
    let decisions = task["decisions"].to_string();
    assert!(
        decisions.contains("PR: drafted by claude-cheap"),
        "{decisions}"
    );
    assert!(
        decisions.contains(&format!("PR: opened {URL}")),
        "{decisions}"
    );
    let brief = std::fs::read_to_string(s.tools.path().join("draft-brief.md")).unwrap();
    assert!(
        brief.contains("CONTRIBUTING.md") && brief.contains("Add pr file"),
        "{brief}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_drafted_name_that_exists_on_the_remote_falls_back_to_the_tasks_branch() {
    let reply = "{\"branch\":\"feature/taken\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup(reply, true);
    git_out(
        s.repo.path(),
        &["push", "-q", "origin", "main:refs/heads/feature/taken"],
    );
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    let branch = task["branch"].as_str().unwrap().to_string();
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(log.contains(&format!("--head {branch}")), "{log}");
    assert!(!log.contains("--head feature/taken"), "{log}");
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task["decisions"]
            .to_string()
            .contains("feature/taken exists on the remote"),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_empty_drafted_name_opens_the_pull_request_on_the_tasks_branch() {
    let reply = "{\"branch\":\"\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup(reply, true);
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    let branch = task["branch"].as_str().unwrap().to_string();
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    assert!(
        s.gh_log().contains(&format!("--head {branch}")),
        "{}",
        s.gh_log()
    );
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task["decisions"]
            .to_string()
            .contains("the draft named none"),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn no_pull_request_is_opened_while_the_setting_is_off() {
    let s = setup("{}", false);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    stays("no gh call while the setting is off", || {
        s.gh_log().is_empty()
    });
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(task.get("prUrl").is_none(), "{task}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_subtask_never_opens_a_pull_request_only_its_top_level_parent_does() {
    let reply = "{\"branch\":\"feature/x\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup(reply, true);
    let parent = s.create(serde_json::json!({"start": false}));
    let child = s.create(serde_json::json!({"parent": parent, "title": "Child", "start": true}));
    done(&s, &child);
    // The parent finishes once its only child landed and opens its own pull request.
    assert_eq!(s.pr_url(&parent).as_deref(), Some(URL), "{}", s.gh_log());
    stays("the subtask opening no pull request", || {
        s.gh_log().matches("pr create").count() == 1
    });
    let child_task = s
        .daemon
        .request("task.get", serde_json::json!({"id": child}));
    assert!(child_task.get("prUrl").is_none(), "{child_task}");
    assert!(
        !child_task["decisions"].to_string().contains("PR:"),
        "{child_task}"
    );
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_unparseable_draft_opens_the_pull_request_with_the_tasks_own_title_and_branch() {
    let s = setup("I could not decide.", true);
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    let branch = task["branch"].as_str().unwrap().to_string();
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(
        log.contains(&format!(
            "pr create --base main --head {branch} --title Add pr file"
        )),
        "{log}"
    );
    assert!(log.contains("- [x] pr.txt exists"), "{log}");
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task["decisions"].to_string().contains("PR: draft failed"),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn settings_refuse_a_pr_route_that_is_no_configured_route() {
    let s = setup("{}", false);
    let mut settings = s.daemon.request("settings.get", serde_json::json!({}));
    settings["prRoute"] = serde_json::json!("nope");
    let err = s
        .daemon
        .request_error("settings.set", serde_json::json!({"settings": settings}));
    assert!(err.contains("prRoute"), "{err}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_task_that_lands_opens_no_pull_request() {
    let s = setup_with("{}", true, false, true);
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    assert!(task["landedSha"].is_string(), "{task}");
    stays("a landed task opening no pull request", || {
        s.gh_log().is_empty()
    });
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task.get("prUrl").is_none() && task.get("prPending").is_none(),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_body_claim_the_diff_contradicts_is_corrected_before_the_push() {
    let reply = "{\"branch\":\"\",\"title\":\"t\",\"body\":\"Adds tests for the toggle\"}";
    let s = setup(reply, true);
    std::fs::write(
        s.tools.path().join("truth-reply"),
        truth_reply(serde_json::json!({
            "ok": false,
            "corrections": ["The diff adds no tests."],
            "body": "Corrected body"
        })),
    )
    .unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(log.contains("--body Corrected body"), "{log}");
    assert!(!log.contains("Adds tests for the toggle"), "{log}");
    assert!(
        s.tools.path().join("checks.log").exists(),
        "no body check ran"
    );
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task["decisions"]
            .to_string()
            .contains("PR: body check: The diff adds no tests."),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_pull_request_whose_head_is_not_the_pushed_commit_is_not_recorded_as_opened() {
    let s = setup_full(
        &script("{}", false),
        Pr::Default,
        false,
        FAKE_GH,
        &[FAST_RETRY],
    );
    std::fs::write(s.tools.path().join("stale-head"), "").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened after 5 tries");
    assert!(task.get("prUrl").is_none(), "{task}");
    assert!(
        task["decisions"].to_string().contains("not the pushed"),
        "{task}"
    );
    wait_notice(&s, &id);
    stays("five pr view calls and one notice", || {
        s.gh_log().matches("pr view").count() == 5 && notices(&s, &id) == 1
    });
    assert_eq!(s.gh_log().matches("pr view").count(), 5, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 1);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_pull_request_opened_while_the_draft_runs_is_not_opened_twice() {
    let s = setup_with(
        "{\"branch\":\"feature/other\",\"title\":\"t\",\"body\":\"b\"}",
        true,
        true,
        false,
    );
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s, "draft-started");
    let opened = s.daemon.request("task.pr", serde_json::json!({"id": id}));
    assert_eq!(opened["prUrl"], URL, "{opened}");
    // A different head for the same task is refused while that one is open.
    let err = s.daemon.request_error(
        "task.pr",
        serde_json::json!({"id": id, "branch": "feature/second"}),
    );
    assert!(err.contains("already open"), "{err}");
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    let task = wait_decision(&s, &id, "draft not used");
    assert!(task.get("prPending").is_none(), "{task}");
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn default_settings_and_no_overseer_open_the_pull_request_of_a_refused_landing() {
    let reply = "{\"branch\":\"feature/by-default\",\"title\":\"feat: by default\",\"body\":\"b\"}";
    let s = setup_full(&script(reply, false), Pr::Default, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(log.contains("--title feat: by default"), "{log}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_task_with_land_off_gets_a_pull_request_too() {
    let s = setup_full(&script("{}", false), Pr::Default, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({"land": false}));
    let task = done(&s, &id);
    assert!(task.get("landedSha").is_none(), "{task}");
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_unreviewed_task_is_not_landed_and_its_pull_request_opens_with_the_unreviewed_line() {
    // The fake answers a review brief with a report and no verdict, so no
    // route reads the attempt, even with landing on.
    let reply = "{\"branch\":\"\",\"title\":\"\",\"body\":\"\"}";
    let s = setup_with(reply, true, false, true);
    let mut settings = s.daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    s.daemon
        .request("settings.set", serde_json::json!({"settings": settings}));
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    assert_eq!(task["unreviewed"], true, "{task}");
    assert!(task.get("landedSha").is_none(), "{task}");
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(log.contains("pr create"), "{log}");
    assert!(
        log.contains("Unreviewed: no review route gave a verdict"),
        "{log}"
    );
    s.daemon.shutdown_and_wait();
}
