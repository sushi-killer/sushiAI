use super::*;

#[test]
fn a_restart_resumes_the_pull_request_of_a_done_task() {
    let reply = "{\"branch\":\"\",\"title\":\"t\",\"body\":\"b\"}";
    let mut s = setup_with(reply, true, true, false);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s, "draft-started");
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(task["prPending"], true, "{task}");
    let branch = task["branch"].as_str().unwrap().to_string();
    let fresh = fake_harness_script(s.tools.path(), "fake-claude-2.sh", &script(reply, false));
    let gh = s.tools.path().join("fake-gh.sh");
    s.daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", fresh.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    assert!(
        s.gh_log().contains(&format!("--head {branch}")),
        "{}",
        s.gh_log()
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_old_stored_plan_on_the_tasks_own_branch_is_drafted_again() {
    let mut s = setup_with(DRAFT, true, true, false);
    let (id, branch) = held_task(&s);
    let old = serde_json::json!({"branch": branch, "title": "", "body": ""});
    restart_with_plan(&mut s, &id, old, DRAFT);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(
        log.contains("--head feature/drafted-name --title feat: drafted title"),
        "{log}"
    );
    assert!(!log.contains(&format!("--head {branch}")), "{log}");
    assert_eq!(drafts(&s), 2, "the held run and the redraft");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_drafted_stored_plan_is_reused_without_a_new_draft() {
    let mut s = setup_with(DRAFT, true, true, false);
    let (id, _) = held_task(&s);
    let plan = serde_json::json!({
        "branch": "fix/stored-name", "title": "fix: stored", "body": "Stored body",
        "branchFrom": "drafted"
    });
    restart_with_plan(&mut s, &id, plan, DRAFT);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(
        log.contains("--head fix/stored-name --title fix: stored --body Stored body"),
        "{log}"
    );
    assert_eq!(drafts(&s), 1, "only the held first run drafted");
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_owner_stored_plan_wins_over_a_new_draft() {
    let mut s = setup_with(DRAFT, true, true, false);
    let (id, _) = held_task(&s);
    let plan = serde_json::json!({
        "branch": "chore/owner-name", "title": "", "body": "", "branchFrom": "owner"
    });
    restart_with_plan(&mut s, &id, plan, DRAFT);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    assert!(
        s.gh_log().contains("--head chore/owner-name"),
        "{}",
        s.gh_log()
    );
    assert_eq!(drafts(&s), 1, "only the held first run drafted");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_restart_reuses_the_saved_draft_instead_of_drafting_again() {
    let reply = "{\"branch\":\"fix/kept-name\",\"title\":\"t\",\"body\":\"b\"}";
    let mut s = setup_full(&script(reply, false), Pr::Default, false, GH_FAILING, &[]);
    std::fs::write(s.tools.path().join("fail-first"), "1").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    // The first try fails and the flow sleeps with the draft saved.
    wait_decision(&s, &id, "PR: try 1 failed");
    let drafts = |s: &Setup| {
        std::fs::read_to_string(s.tools.path().join("drafts.log"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    assert_eq!(drafts(&s), 1);
    let other = s.tools.path().join("fake-claude-b.sh");
    std::fs::copy(s.tools.path().join("fake-claude.sh"), &other).unwrap();
    let gh = s.tools.path().join("fake-gh.sh");
    s.daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", other.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    assert_eq!(drafts(&s), 1, "drafted again after the restart");
    // The retry and the restart keep the first chosen name.
    assert!(
        s.gh_log().contains("--head fix/kept-name"),
        "{}",
        s.gh_log()
    );
    s.daemon.shutdown_and_wait();
}
