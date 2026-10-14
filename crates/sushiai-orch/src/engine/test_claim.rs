//! An implementer's claim that a protected planner test is wrong. The test is
//! restored whatever the claim says; a fresh read-only judge on the planner's
//! route checks the claim against the task goal. Confirmed: the planner (not
//! the implementer) rewrites the test and orchd commits it into the planner
//! tests. Rejected: the agent carries on. Unclear: one owner question.

use super::*;

const MAX_WHY_CHARS: usize = 300;

const KEEP_TEST: &str = "Keep the test";
const REWRITE_TEST: &str = "Rewrite the test";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Confirmed,
    Rejected,
    Unclear,
}

/// The judge's reply: verdict and a one-line reason.
pub(super) fn parse_ruling(text: &str) -> Option<(Verdict, String)> {
    let v: serde_json::Value =
        serde_json::from_str(&brief::tagged_json(text, "sushi-test-ruling")?).ok()?;
    let verdict = match v
        .get("verdict")?
        .as_str()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "confirmed" => Verdict::Confirmed,
        "rejected" => Verdict::Rejected,
        "unclear" => Verdict::Unclear,
        _ => return None,
    };
    let why = v
        .get("why")
        .and_then(|w| w.as_str())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some((verdict, truncate_chars(&why, MAX_WHY_CHARS)))
}

/// The planner's rewritten file; `None` when empty or over the size cap.
pub(super) fn parse_rewrite(text: &str) -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_str(&brief::tagged_json(text, "sushi-test")?).ok()?;
    let content = v.get("content")?.as_str()?.to_string();
    (!content.trim().is_empty() && content.len() <= MAX_TEST_BYTES).then_some(content)
}

fn context(task: &Task, path: &str, content: &str, reason: &str) -> String {
    let criteria = task
        .criteria
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n{}\n{}\n{}\n",
        brief::untrusted_block("The task goal", &task.goal),
        brief::untrusted_block("The acceptance criteria", &criteria),
        brief::untrusted_block(&format!("The test {path}"), content),
        brief::untrusted_block("The implementer's claim", reason),
    )
}

/// The judge's brief: is the test at `path` (now `content`) wrong, as the
/// implementer's `reason` claims?
pub(super) fn judge_brief(task: &Task, path: &str, content: &str, reason: &str) -> String {
    format!(
        "{}\n\n{}{}\n",
        engine_text("test_claim_judge"),
        context(task, path, content, reason),
        engine_text("test_claim_judge_reply")
    )
}

/// The planner's brief to rewrite the test at `path` the judge found wrong.
pub(super) fn rewrite_brief(task: &Task, path: &str, content: &str, reason: &str) -> String {
    format!(
        "{}\n\n{}{}\n",
        engine_text("test_claim_rewrite"),
        context(task, path, content, reason),
        engine_text("test_claim_rewrite_reply")
    )
}

/// One read-only run on the planner's route; its final text.
async fn planner_run(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    kind: &'static str,
    brief_text: &str,
    worktree: &Path,
    cancel: &CancelToken,
) -> Result<String, String> {
    let settings = app.settings.read().unwrap().clone();
    let route = settings
        .routes
        .iter()
        .find(|r| r.id == task.variant().plan_route_id(&settings))
        .cloned()
        .ok_or("no planner route")?;
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, kind, Some(attempt_n))
        .map_err(|e| e.to_string())?;
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let _ = std::fs::write(run_dir.join("brief.md"), brief_text);
    let events_path = run_dir.join("events.jsonl");
    let req = harness::RunRequest {
        repo_settings: false,
        resume: None,
        ..review_request(&route, worktree, &mcp_path, &settings_path, &[])
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        RunTrack::No,
        worktree,
        &req,
        CostTag::task("finding_judge", &route.id),
        brief_text,
        &events_path,
        cancel,
        Guard::helper(kind, limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = result.map_err(|_| "the run was stopped or did not start".to_string())?;
    task.cost_usd += outcome.cost_usd.unwrap_or(0.0);
    if outcome.timed_out {
        return Err(helper_timeout_message(kind, limit));
    }
    if let Some(error) = outcome.error {
        return Err(format!("errored: {}", head_chars(&error, 200)));
    }
    Ok(outcome.final_text.unwrap_or_default())
}

/// A throwaway checkout next to the task's worktree. It must stay outside the
/// data dir, which the read-only runs may not read: that would include their
/// own working directory.
fn scratch_path(task: &Task, name: &str) -> PathBuf {
    PathBuf::from(format!("{}-{name}", task.worktree))
}

/// The tests-first rule for a rewrite: on a clean checkout of the base with
/// `files` written, every check that gated the task before must still fail.
async fn still_fails_on_base(
    app: &Arc<App>,
    task: &Task,
    files: &[PlannedTest],
    cancel: &CancelToken,
) -> Result<(), String> {
    let gated = gated_checks(task);
    if gated.is_empty() {
        return Err("no check fails on the base, so the rewrite cannot be validated".into());
    }
    let repo = PathBuf::from(&task.repo);
    let run_dir = app.store.task_dir(&task.id).join("runs/claim-base");
    let path = scratch_path(task, "claim-base");
    let (r, p, sha) = (repo.clone(), path.clone(), task.base_sha.clone());
    let made = tokio::task::spawn_blocking(move || {
        git::remove_worktree(&r, &p);
        git::add_detached_worktree(&r, &p, &sha)?;
        let _ = git::bootstrap_worktree(&r, &p);
        Ok::<(), git::GitError>(())
    })
    .await;
    if !matches!(made, Ok(Ok(()))) {
        return Err("could not check out the base".into());
    }
    let result = async {
        for f in files {
            let target = path.join(&f.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&target, &f.content).map_err(|e| e.to_string())?;
        }
        let sandbox = app.task_sandbox(&task.repo, &path, &run_dir);
        worktrees::setup_worktree(&repo, &path, sandbox.as_ref(), cancel).await?;
        let timeout = Duration::from_secs(app.settings.read().unwrap().verify_timeout_secs);
        for check in &gated {
            let out = run_one_verify_command(
                CheckContext::new(app, Some(&task.id)),
                &path,
                &check.run,
                timeout,
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            )
            .await;
            if classify_baseline(&out) != Some(Baseline::Fail) {
                return Err(format!(
                    "the rewritten test no longer fails on the base: `{}` does not fail there",
                    check.run
                ));
            }
        }
        Ok(())
    }
    .await;
    let (r, p) = (repo, path);
    let _ = tokio::task::spawn_blocking(move || git::remove_worktree(&r, &p)).await;
    result
}

/// The only gate a planner rewrite passes: it must change the file and the
/// checks that gated the task must still fail on the base. Returns the test
/// files with the rewrite in place.
async fn accept_rewrite(
    app: &Arc<App>,
    task: &Task,
    idx: usize,
    new: String,
    cancel: &CancelToken,
) -> Result<Vec<PlannedTest>, String> {
    if new == task.planner_tests.files[idx].content {
        return Err("the planner returned the test unchanged".into());
    }
    let mut files = task.planner_tests.files.clone();
    files[idx].content = new;
    still_fails_on_base(app, task, &files, cancel).await?;
    Ok(files)
}

/// Rewrites `path` through the planner and stores the new file with the task
/// and in the worktree; the decision line says what changed and why. `scratch`
/// is a checkout of the base with the tests, not the agent's worktree.
#[allow(clippy::too_many_arguments)]
async fn rewrite_test(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    path: &str,
    reason: &str,
    why: &str,
    worktree: &Path,
    scratch: &Path,
    cancel: &CancelToken,
) -> Result<(), String> {
    let idx = task
        .planner_tests
        .files
        .iter()
        .position(|t| t.path == path)
        .ok_or("not a planner test")?;
    let old = task.planner_tests.files[idx].content.clone();
    let brief_text = rewrite_brief(task, path, &old, reason);
    let text = planner_run(
        app,
        task,
        attempt_n,
        "test-rewrite",
        &brief_text,
        scratch,
        cancel,
    )
    .await?;
    let new = parse_rewrite(&text).ok_or("the planner gave no usable file")?;
    let files = accept_rewrite(app, task, idx, new.clone(), cancel).await?;
    let (wt, written) = (worktree.to_path_buf(), files.clone());
    tokio::task::spawn_blocking(move || write_files(&wt, &written))
        .await
        .unwrap_or_else(|e| Err(e.to_string()))?;
    task.planner_tests.files = files;
    task.planner_tests.protected = Some(protected_hashes(worktree, &task.base_sha, task));
    task.decisions.push(format!(
        "Orchestrator: the planner rewrote protected test {path} ({} -> {} lines) after the agent's claim ({}); judge: {why}",
        old.lines().count(),
        new.lines().count(),
        truncate_chars(reason, 200)
    ));
    Ok(())
}

/// Rules on every claim of attempt `idx`: once per planner test file.
#[allow(clippy::too_many_arguments)]
pub(super) async fn rule_on_claims(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    claims: &[brief::TestClaim],
    worktree: &Path,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) {
    let attempt_n = task.attempts[idx].n;
    let task_id = task.id.clone();
    // The judge and the planner read a checkout of the base with the planner tests, never
    // the agent's worktree: what the agent wrote there is not an instruction.
    let scratch = scratch_path(task, "claim-tests");
    let mut scratch_made = false;
    for claim in claims {
        let Ok(path) = clean_test_path(&claim.path) else {
            continue;
        };
        let Some(current) = task
            .planner_tests
            .files
            .iter()
            .find(|t| t.path == path)
            .map(|t| t.content.clone())
        else {
            task.decisions.push(format!(
                "Orchestrator: the claim that {path} is wrong was not ruled on: it is not one of the planner's tests"
            ));
            continue;
        };
        if task.planner_tests.ruled.contains(&path) {
            continue;
        }
        task.planner_tests.ruled.push(path.clone());
        task.updated_at = now_ms();
        app.save_task_logged(task);
        if !scratch_made {
            let (r, p, sha, files) = (
                PathBuf::from(&task.repo),
                scratch.clone(),
                task.base_sha.clone(),
                task.planner_tests.files.clone(),
            );
            let made = tokio::task::spawn_blocking(move || {
                git::remove_worktree(&r, &p);
                git::add_detached_worktree(&r, &p, &sha).map_err(|e| e.to_string())?;
                write_files(&p, &files).map(|_| ())
            })
            .await;
            if !matches!(made, Ok(Ok(()))) {
                task.decisions.push(format!(
                    "Orchestrator: the claim that {path} is wrong was not ruled on (no clean checkout of the tests); the test stays"
                ));
                continue;
            }
            scratch_made = true;
        }
        let brief_text = judge_brief(task, &path, &current, &claim.reason);
        let ruling = planner_run(
            app,
            task,
            attempt_n,
            "test-judge",
            &brief_text,
            &scratch,
            cancel,
        )
        .await
        .and_then(|text| parse_ruling(&text).ok_or_else(|| "no verdict".to_string()));
        let (verdict, why) = match ruling {
            Ok(r) => r,
            Err(e) => {
                task.decisions.push(format!(
                    "Orchestrator: the claim that {path} is wrong was not ruled on ({e}); the test stays"
                ));
                continue;
            }
        };
        let rewrite = match verdict {
            Verdict::Confirmed => true,
            Verdict::Rejected => {
                task.decisions.push(format!(
                    "Orchestrator: the claim that {path} is wrong was rejected: {why}"
                ));
                false
            }
            Verdict::Unclear => {
                task.question = Some(Question::new(
                    format!(
                        "The agent says {path} is wrong: {}\nThe judge could not settle it: {why}",
                        truncate_chars(&claim.reason, 400)
                    ),
                    vec![REWRITE_TEST.to_string(), KEEP_TEST.to_string()],
                    QuestionKind::AgentQuestion,
                    AskedBy::Implement,
                ));
                task.status = TaskStatus::Waiting;
                task.updated_at = now_ms();
                let answer =
                    wait_for_answer(app, &task_id, task, pending_answer, cancel, permit).await;
                answer.is_some_and(|a| a.trim().eq_ignore_ascii_case(REWRITE_TEST))
            }
        };
        if rewrite {
            if let Err(e) = rewrite_test(
                app,
                task,
                attempt_n,
                &path,
                &claim.reason,
                &why,
                worktree,
                &scratch,
                cancel,
            )
            .await
            {
                task.decisions.push(format!(
                    "Orchestrator: rewriting {path} failed ({e}); the test stays"
                ));
            }
        }
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    if scratch_made {
        let (r, p) = (PathBuf::from(&task.repo), scratch);
        let _ = tokio::task::spawn_blocking(move || git::remove_worktree(&r, &p)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_judge_checkout_is_not_under_a_path_the_run_may_not_read() {
        let (app, _dir) = test_app();
        let repo = app.home.join("some-repo");
        let mut task = task_with_status(TaskStatus::Running);
        task.worktree = git::worktree_path(&repo, &app.home, "", "task/x")
            .to_string_lossy()
            .to_string();
        let deny_read = PathBuf::from(&app.data_dir);
        for name in ["claim-tests", "claim-base"] {
            assert!(!scratch_path(&task, name).starts_with(&deny_read));
            assert_eq!(
                scratch_path(&task, name).parent(),
                Path::new(&task.worktree).parent()
            );
        }
        // The old place, inside the task's data dir, would have been denied.
        assert!(app
            .store
            .task_dir(&task.id)
            .join("runs/claim-tests")
            .starts_with(&deny_read));
    }

    /// A task gated by `sh t/a.test`, which fails on the base: the base has
    /// no `answer.txt`. `old` is the planner's test file as committed.
    fn gated_task(dir: &tempfile::TempDir, old: &str) -> Task {
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = dir.path().join("task").to_string_lossy().to_string();
        task.base_sha = git::head_sha(&repo).unwrap();
        task.criteria = vec!["the answer is written".into()];
        task.checks = vec![Check {
            criterion: 0,
            run: "sh t/a.test".into(),
            baseline: Some(Baseline::Fail),
        }];
        task.planner_tests.files = vec![PlannedTest {
            path: "t/a.test".into(),
            content: old.into(),
        }];
        task
    }

    #[tokio::test]
    async fn a_shorter_rewrite_that_still_fails_on_the_base_is_accepted() {
        let (app, dir) = test_app_fitting_sandbox();
        let old = "#!/bin/sh\nset -e\n# the file must exist and hold the answer\ntest -f answer.txt\ntest -s answer.txt\ngrep -q 42 answer.txt\necho checked\n";
        let task = gated_task(&dir, old);
        // Well under half of the original's lines, and it fails on the base.
        let new = "test -f answer.txt\n".to_string();
        let files = accept_rewrite(&app, &task, 0, new.clone(), &CancelToken::new())
            .await
            .unwrap();
        assert_eq!(files[0].content, new);
    }

    #[tokio::test]
    async fn a_rewrite_that_passes_on_the_base_is_refused_whatever_its_length() {
        let (app, dir) = test_app_fitting_sandbox();
        let old = "#!/bin/sh\nset -e\n# the file must exist and hold the answer\ntest -f answer.txt\ntest -s answer.txt\ngrep -q 42 answer.txt\necho checked\n";
        let task = gated_task(&dir, old);
        // Same number of lines as the original, and it passes on the base.
        let same_length =
            "#!/bin/sh\nset -e\n# nothing is checked here\ntrue\ntrue\ntrue\necho checked\n";
        let shorter = "true\n";
        for new in [same_length, shorter] {
            let err = accept_rewrite(&app, &task, 0, new.to_string(), &CancelToken::new())
                .await
                .unwrap_err();
            assert!(err.contains("no longer fails on the base"), "{err}");
        }
    }
}
