use super::*;

const CHECK_PROMPT: &str = "You check a coding task brief before an agent works on it. Decide whether any two requirements below (the goal and the acceptance criteria) contradict each other, so that satisfying one makes another impossible. Ordinary tension or extra work is not a contradiction. Reply with only JSON: {\"contradiction\": true|false, \"conflict\": \"<which two requirements, quoted briefly, or empty>\"}";

const MAX_CONFLICT_CHARS: usize = 400;

/// What the caller does after a check.
pub(super) enum BriefAction {
    Proceed,
    /// The planner's draft contradicts itself: draft again, once.
    Redraft,
    /// The task was cancelled while the check ran.
    Cancelled,
}

enum Verdict {
    Clear,
    Conflict(String),
    Unusable(String),
}

pub(super) fn build_check_brief(goal: &str, criteria: &[String]) -> String {
    format!(
        "{CHECK_PROMPT}\n\n{}\n",
        json!({"goal": goal, "criteria": criteria})
    )
}

/// The reply's JSON object, whatever prose or fence surrounds it. A
/// contradiction with no named conflict still counts, with a generic text.
fn parse_verdict(text: &str) -> Verdict {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Verdict::Unusable("the reply had no JSON".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[start..=end]) else {
        return Verdict::Unusable("the reply was not valid JSON".to_string());
    };
    match v.get("contradiction").and_then(|c| c.as_bool()) {
        Some(false) => Verdict::Clear,
        Some(true) => {
            let conflict = v
                .get("conflict")
                .and_then(|c| c.as_str())
                .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            Verdict::Conflict(if conflict.is_empty() {
                "two requirements contradict each other".to_string()
            } else {
                truncate_chars(&conflict, MAX_CONFLICT_CHARS)
            })
        }
        None => Verdict::Unusable("the reply had no contradiction flag".to_string()),
    }
}

async fn run_check(
    app: &Arc<App>,
    task: &Task,
    route: &Route,
    attempt_n: u32,
    cancel: &CancelToken,
) -> Result<Verdict, RunError> {
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(&task.id, attempt_n).join("brief-check");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let brief_text = build_check_brief(&task.goal, &task.criteria);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let req = harness::RunRequest {
        repo_settings: false,
        ..review_request(route, &worktree, &mcp_path, &settings_path, &[])
    };
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        false,
        &worktree,
        &req,
        CostTag::task("brief_check", &route.id),
        &brief_text,
        &events_path,
        cancel,
        None,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = match result {
        Ok(o) => o,
        Err(RunError::Cancelled) => {
            // What it streamed before the stop is still spent.
            if let Some(cost) =
                replay_run_cost(&events_path, &settings.prices).and_then(|o| o.cost_usd)
            {
                add_task_cost(app, &task.id, cost);
            }
            return Err(RunError::Cancelled);
        }
        Err(e) => return Err(e),
    };
    add_task_cost(app, &task.id, outcome.cost_usd.unwrap_or(0.0));
    if let Some(error) = outcome.error {
        return Ok(Verdict::Unusable(format!("the run failed ({error})")));
    }
    Ok(parse_verdict(&outcome.final_text.unwrap_or_default()))
}

fn add_task_cost(app: &Arc<App>, task_id: &str, cost: f64) {
    if cost <= 0.0 {
        return;
    }
    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
        t.cost_usd += cost;
        t.updated_at = now_ms();
        let _ = app.store.save_task(&t);
        app.broadcast_task(&t);
    }
}

/// Checks the stored task's goal and criteria once. `can_redraft` is true
/// for a planner draft: a contradiction then asks the planner to draft again
/// (at most once per task). Otherwise, and for an unusable reply, the task
/// carries on; a conflict is kept for the implement and review briefs.
pub(super) async fn check_brief(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    can_redraft: bool,
    cancel: &CancelToken,
) -> BriefAction {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    if task.brief_check.done {
        return BriefAction::Proceed;
    }
    let route_id = app.settings.read().unwrap().brief_check_route.clone();
    if route_id.is_empty() {
        return BriefAction::Proceed;
    }
    let route = app
        .settings
        .read()
        .unwrap()
        .routes
        .iter()
        .find(|r| r.id == route_id)
        .cloned()
        // Settings saved before this route existed still reach the built-in one.
        .or_else(|| {
            Settings::default()
                .routes
                .into_iter()
                .find(|r| r.id == route_id)
        });
    let verdict = match &route {
        Some(route) => match run_check(app, &task, route, attempt_n, cancel).await {
            Ok(v) => v,
            Err(RunError::Cancelled) => return BriefAction::Cancelled,
            Err(RunError::Io(msg)) => Verdict::Unusable(msg),
        },
        None => Verdict::Unusable(format!("route {route_id} is not configured")),
    };
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    let mut action = BriefAction::Proceed;
    match verdict {
        Verdict::Clear => task
            .decisions
            .push("Brief check: no contradiction found".to_string()),
        Verdict::Unusable(why) => task.decisions.push(format!(
            "Brief check: no usable answer ({}); continuing",
            truncate_chars(&why, 200)
        )),
        Verdict::Conflict(conflict) => {
            if can_redraft && !task.brief_check.redrafted {
                task.decisions
                    .push(format!("Brief check: {conflict} (drafting again)"));
                task.brief_check.redrafted = true;
                let request = task.request.clone().unwrap_or_default();
                task.request = Some(format!(
                    "{request}\n\n(Brief check: the previous draft's requirements contradict each other: {conflict}. Draft again so that no two requirements conflict.)"
                ));
                task.status = TaskStatus::Drafting;
                action = BriefAction::Redraft;
            } else {
                task.decisions.push(format!("Brief check: {conflict}"));
                task.brief_check.conflict = Some(conflict);
            }
        }
    }
    // A redraft is checked again; anything else is settled for good.
    task.brief_check.done = !matches!(action, BriefAction::Redraft);
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_is_read_from_json_inside_prose_or_a_fence() {
        let text = "```json\n{\"contradiction\": true, \"conflict\": \"A vs B\"}\n```";
        assert!(matches!(parse_verdict(text), Verdict::Conflict(c) if c == "A vs B"));
        assert!(matches!(
            parse_verdict("{\"contradiction\": false, \"conflict\": \"\"}"),
            Verdict::Clear
        ));
    }

    #[test]
    fn a_reply_without_a_flag_is_unusable() {
        assert!(matches!(parse_verdict("looks fine"), Verdict::Unusable(_)));
        assert!(matches!(
            parse_verdict("{\"conflict\": \"x\"}"),
            Verdict::Unusable(_)
        ));
    }

    #[test]
    fn a_contradiction_without_text_still_counts() {
        assert!(matches!(
            parse_verdict("{\"contradiction\": true}"),
            Verdict::Conflict(c) if !c.is_empty()
        ));
    }

    #[test]
    fn the_check_brief_carries_goal_and_criteria_as_json() {
        let b = build_check_brief("g", &["a".to_string()]);
        assert!(b.starts_with("You check a coding task brief"));
        assert!(b.contains(r#"{"criteria":["a"],"goal":"g"}"#));
    }
}
