//! An implementer's claim that a protected planner test is wrong. The test is
//! restored whatever the claim says; a fresh read-only judge on the planner's
//! route checks the claim against the task goal. Confirmed: the planner (not
//! the implementer) rewrites the test and orchd commits it into the planner
//! tests. Rejected: the agent carries on. Unclear: one owner question.

use super::*;

const MAX_WHY_CHARS: usize = 300;

const JUDGE_PROMPT: &str = "An implementer says one of the task's acceptance tests is wrong. You may read files in the working directory but change nothing. Check the claim against the task goal, the criteria and the test itself. Answer `confirmed` when the test contradicts the goal or the criteria or cannot pass for a correct implementation; `rejected` when the test is right and the implementation has to change; `unclear` when the goal does not settle it.";

const JUDGE_REPLY: &str = "Reply with only:\n```sushi-test-ruling\n{\"verdict\": \"confirmed|rejected|unclear\", \"why\": \"<one sentence>\"}\n```";

const REWRITE_PROMPT: &str = "Rewrite one acceptance test of the task. The implementer claims it is wrong and a judge agreed. You may read files in the working directory but change nothing. Keep what the test is meant to check according to the task goal and criteria; fix only what is wrong. Reply with the complete new file content.";

const REWRITE_REPLY: &str =
    "Reply with only:\n```sushi-test\n{\"content\": \"<the complete new file>\"}\n```";

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

/// One read-only run on the planner's route; its final text.
async fn planner_run(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    kind: &'static str,
    brief_text: &str,
    cancel: &CancelToken,
) -> Result<String, String> {
    let settings = app.settings.read().unwrap().clone();
    let route = settings
        .routes
        .iter()
        .find(|r| r.id == task.variant().plan_route_id(&settings))
        .cloned()
        .ok_or("no planner route")?;
    let worktree = PathBuf::from(&task.worktree);
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
        ..review_request(&route, &worktree, &mcp_path, &settings_path, &[])
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        RunTrack::No,
        &worktree,
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

/// Rewrites `path` through the planner and commits it into the planner
/// tests; the decision line says what changed and why.
async fn rewrite_test(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    path: &str,
    reason: &str,
    why: &str,
    cancel: &CancelToken,
) -> Result<(), String> {
    let idx = task
        .planner_tests
        .files
        .iter()
        .position(|t| t.path == path)
        .ok_or("not a planner test")?;
    let old = task.planner_tests.files[idx].content.clone();
    let brief_text = format!(
        "{REWRITE_PROMPT}\n\n{}{REWRITE_REPLY}\n",
        context(task, path, &old, reason)
    );
    let text = planner_run(app, task, attempt_n, "test-rewrite", &brief_text, cancel).await?;
    let new = parse_rewrite(&text).ok_or("the planner gave no usable file")?;
    let worktree = PathBuf::from(&task.worktree);
    let (wt, rel, body) = (worktree.clone(), path.to_string(), new.clone());
    let (id, title) = (task.id.clone(), task.title.clone());
    let sha = tokio::task::spawn_blocking(move || {
        std::fs::write(wt.join(&rel), body).map_err(|e| e.to_string())?;
        git::commit_paths(&wt, &[rel], &planner_tests_title(&title), &id).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))?;
    task.planner_tests.files[idx].content = new.clone();
    task.planner_tests.commit = Some(sha.clone());
    task.planner_tests.protected = Some(protected_hashes(&worktree, &sha, task));
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
        let _ = worktree;
        let brief_text = format!(
            "{JUDGE_PROMPT}\n\n{}{JUDGE_REPLY}\n",
            context(task, &path, &current, &claim.reason)
        );
        let ruling = planner_run(app, task, attempt_n, "test-judge", &brief_text, cancel)
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
            if let Err(e) =
                rewrite_test(app, task, attempt_n, &path, &claim.reason, &why, cancel).await
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
}
