use super::*;

/// How long the report-only turn may run.
const REPORT_TURN_MINS: f64 = 3.0;

/// What an attempt that ran out of time leaves for the next one.
pub(super) struct BudgetReport {
    pub handoff: String,
    pub summary: Option<String>,
    /// What the report turn cost; `0.0` when it did not run.
    pub cost: f64,
}

/// The run was stopped at its wall-clock budget. Resumes its session once,
/// read-only, for a report-only turn: no tools to change anything, no MCP,
/// no Stop hook. Without a session, or when the turn fails or reports no
/// handoff, the handoff names the budget and the changed files instead.
#[allow(clippy::too_many_arguments)]
pub(super) async fn report_after_budget(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    route: &Route,
    worktree: &Path,
    run_dir: &Path,
    session: Option<&str>,
    changed_files: &[String],
    cancel: &CancelToken,
) -> BudgetReport {
    let mins = app.settings.read().unwrap().implement_budget_mins;
    let mut cost = 0.0;
    let mut report = None;
    if let Some(session) = session {
        let req = harness::RunRequest {
            harness: route.harness,
            worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            max_budget_usd: None,
            review: true,
            task_run: false,
            task_sandbox: None,
            mcp_config: None,
            settings_path: None,
            network_allowed: false,
            codex_mcp: &[],
            images: &[],
            repo_settings: false,
            resume: Some(session),
        };
        let events = run_dir.join("report.events.jsonl");
        let guard = Guard::off().with_time_budget(REPORT_TURN_MINS, now_ms());
        let prompt = brief::time_budget_report_prompt();
        if let Ok(outcome) = run_harness(
            app,
            task_id,
            attempt_n,
            RunTrack::No,
            worktree,
            &req,
            CostTag::task("implement", &route.id),
            &prompt,
            &events,
            cancel,
            guard,
        )
        .await
        {
            // A resumed Claude session's own total covers the run before it.
            let own = match route.harness {
                Harness::Claude => outcome.streamed_cost(&app.settings.read().unwrap().prices),
                Harness::Codex => None,
            };
            cost = own.or(outcome.cost_usd).unwrap_or(0.0);
            if !outcome.time_budget {
                report = brief::parse_report(&outcome.final_text.unwrap_or_default())
                    .filter(|r| !r.handoff.trim().is_empty());
            }
        }
    }
    match report {
        Some(r) => BudgetReport {
            handoff: r.handoff.trim().to_string(),
            summary: Some(r.summary).filter(|s| !s.trim().is_empty()),
            cost,
        },
        None => BudgetReport {
            handoff: fallback_handoff(mins, changed_files),
            summary: None,
            cost,
        },
    }
}

fn fallback_handoff(mins: f64, changed_files: &[String]) -> String {
    let files = if changed_files.is_empty() {
        "no file changed yet".to_string()
    } else {
        let shown: Vec<&str> = changed_files.iter().take(20).map(String::as_str).collect();
        let more = changed_files.len().saturating_sub(shown.len());
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        format!("changed files: {}{more}", shown.join(", "))
    };
    format!(
        "The attempt was stopped at its {mins} minute time budget before it reported; {files}. Continue from the working tree."
    )
}

#[cfg(test)]
mod tests {
    use super::fallback_handoff;

    #[test]
    fn the_fallback_names_the_budget_and_the_files() {
        let text = fallback_handoff(45.0, &["a.rs".into(), "b.rs".into()]);
        assert!(text.contains("45 minute"));
        assert!(text.contains("a.rs, b.rs"));
        assert!(fallback_handoff(0.05, &[]).contains("no file changed"));
    }
}
