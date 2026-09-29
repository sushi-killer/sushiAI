use super::*;

/// Longest advice kept, in characters.
const MAX_ADVICE_CHARS: usize = 1500;

/// One read-only call on the task's planner route about the implement attempt
/// that just failed, made before the next attempt starts. The answer is
/// stored as the failed attempt's `advice`, and the run's cost as its
/// `advisor_cost_usd`, which is also returned (`None` when no run happened).
/// A failed attempt is advised at most once: a run that was stopped or gave
/// no answer is not paid for again. Never fails the task: no route, an
/// escalation to the advisor's own model, a failed or stopped run or an
/// empty answer all just mean no advice. `force` skips the check that the
/// advisor is not the model about to implement: a repeated review finding
/// gets a fresh diagnosis even when the tier went up to the planner's route.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_advisor_before_retry(
    app: &Arc<App>,
    task: &mut Task,
    settings: &Settings,
    next_route: &Route,
    worktree: &Path,
    base_sha: &str,
    force: bool,
    cancel: &CancelToken,
) -> Option<f64> {
    let failed = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)?;
    let attempt = &task.attempts[failed];
    let failure = attempt.failure.as_ref()?;
    if attempt.status != AttemptStatus::Failed
        || failure.kind == FailureKind::Blocked
        || attempt.advice.is_some()
        || attempt.advisor_cost_usd.is_some()
    {
        return None;
    }
    let variant = task.variant();
    let planner = variant.plan_route_id(settings);
    let advisor = settings.routes.iter().find(|r| r.id == planner)?;
    if !force
        && (advisor.id == next_route.id
            || (advisor.harness == next_route.harness
                && advisor.model.is_some()
                && advisor.model == next_route.model))
    {
        return None;
    }
    let attempt_n = attempt.n;
    let failure_detail = failure.detail.clone();

    let (wt, base) = (worktree.to_path_buf(), base_sha.to_string());
    let diff =
        tokio::task::spawn_blocking(move || git::diff_full(&wt, &base, 20_000).unwrap_or_default())
            .await
            .unwrap_or_default();
    let brief_text = brief::build_advisor_brief(task, &diff, &failure_detail);

    let run_dir = app.store.run_dir(&task.id, attempt_n).join("advisor");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(advisor.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            advisor,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: advisor.harness,
        worktree,
        model: advisor.model.as_deref(),
        effort: advisor.effort.as_deref(),
        max_budget_usd: None,
        review: true,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: None,
        images: &[],
        repo_settings: true,
    };
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        false,
        worktree,
        &req,
        CostTag::task("advisor", &advisor.id),
        &brief_text,
        &events_path,
        cancel,
        None,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    // The run wrote the attempt's session/pgid to disk mid-run; keep the
    // caller's copy and only add the advice.
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => {
            let cost = replay_run_cost(&events_path, &settings.prices)
                .and_then(|o| o.cost_usd)
                .unwrap_or(0.0);
            task.attempts[failed].advisor_cost_usd = Some(cost);
            task.attempts[failed].advisor_fingerprint = read_cancelled_fingerprint(&events_path);
            return Some(cost);
        }
        Err(RunError::Io(_)) => return None,
    };
    let cost = outcome.cost_usd.unwrap_or(0.0);
    task.attempts[failed].advisor_cost_usd = Some(cost);
    task.attempts[failed].advisor_fingerprint = outcome.fingerprint.clone();
    let text = outcome.final_text.unwrap_or_default();
    let advice = text.trim();
    if outcome.error.is_none() && !advice.is_empty() {
        task.attempts[failed].advice = Some(truncate_chars(advice, MAX_ADVICE_CHARS));
    }
    Some(cost)
}
