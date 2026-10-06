use super::*;

const CHECK_PROMPT: &str = "You check a coding task brief before an agent works on it. Two checks.\n1. Decide whether any two requirements below (the goal and the acceptance criteria) contradict each other, so that satisfying one makes another impossible. Ordinary tension or extra work is not a contradiction.\n2. The agent will have only the capabilities listed under `tools`. Decide whether a criterion can only be met with a capability none of them provides, such as moving an Asana task when no Asana tool is listed. Work on files, shell commands and the repository needs no extra tool, and neither does a criterion that only asks the agent to report something.\nReply with only JSON: {\"contradiction\": true|false, \"conflict\": \"<which two requirements, quoted briefly, or empty>\", \"missingTool\": null or {\"criterion\": <0-based index of the criterion>, \"capability\": \"<what is missing, in a few words>\", \"toolId\": \"<id of a listed connected tool that is off for this task and would provide it, or empty>\"}}";

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

pub(super) fn build_check_brief(
    goal: &str,
    criteria: &[String],
    tools: &serde_json::Value,
) -> String {
    format!(
        "{CHECK_PROMPT}\n\n{}\n",
        json!({"goal": goal, "criteria": criteria, "tools": tools})
    )
}

/// The criterion the reply says needs a tool the task will not have; a
/// criterion index outside the list, or a blank capability, is no claim.
fn parse_missing_tool(
    text: &str,
    criteria: usize,
    off: &[(String, String)],
) -> Option<MissingTool> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;
    let m = v.get("missingTool").filter(|m| m.is_object())?;
    let criterion = m.get("criterion")?.as_u64()? as usize;
    let capability = m
        .get("capability")?
        .as_str()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if criterion >= criteria || capability.is_empty() {
        return None;
    }
    let tool_id = m
        .get("toolId")
        .and_then(|t| t.as_str())
        .filter(|id| off.iter().any(|(known, _)| known == id))
        .map(str::to_string);
    Some(MissingTool {
        criterion,
        capability: truncate_chars(&capability, MAX_CONFLICT_CHARS),
        tool_id,
    })
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
) -> Result<(Verdict, Option<MissingTool>), RunError> {
    let settings = app.settings.read().unwrap().clone();
    let tools = permissions::task_tools(app, &settings, task, Path::new(&task.worktree));
    let brief_text = build_check_brief(&task.goal, &task.criteria, &tools.capabilities());
    let (reply, cost) = run_judge(
        app,
        task,
        route,
        attempt_n,
        "brief_check",
        "brief-check",
        &brief_text,
        cancel,
    )
    .await?;
    add_task_cost(app, &task.id, cost);
    Ok(match reply {
        Ok(text) => {
            let missing = parse_missing_tool(&text, task.criteria.len(), &tools.off);
            (parse_verdict(&text), missing)
        }
        Err(why) => (Verdict::Unusable(why), None),
    })
}

/// One cheap read-only run of `route` on `prompt` in the task's worktree,
/// files under `<run dir>/<name>`. Returns the reply (`Err` is a run that
/// ended in an error, `Ok` its final text) and the run's cost, which the
/// caller adds to its own copy of the task; only a cancelled run's partial
/// cost is added to the stored task here.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_judge(
    app: &Arc<App>,
    task: &Task,
    route: &Route,
    attempt_n: u32,
    stage: &'static str,
    name: &str,
    prompt: &str,
    cancel: &CancelToken,
) -> Result<(Result<String, String>, f64), RunError> {
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(&task.id, attempt_n).join(name);
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
    let _ = std::fs::write(run_dir.join("brief.md"), prompt);
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
        CostTag::task(stage, &route.id),
        prompt,
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
    let cost = outcome.cost_usd.unwrap_or(0.0);
    if let Some(error) = outcome.error {
        return Ok((Err(format!("the run failed ({error})")), cost));
    }
    Ok((Ok(outcome.final_text.unwrap_or_default()), cost))
}

pub(super) fn add_task_cost(app: &Arc<App>, task_id: &str, cost: f64) {
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

/// The `briefCheckRoute` id, the route it resolves to (`None` when no route
/// on an available harness exists) and, when the id is not usable and the
/// cheapest available route stands in, the reason to record; `None` overall
/// when the setting is empty, i.e. cheap checks are off. `"auto"` names no
/// route: it is the cheapest configured route whose harness is available.
pub(super) fn check_route(app: &App) -> Option<(String, Option<Route>, Option<String>)> {
    let settings = app.settings.read().unwrap();
    let route_id = settings.brief_check_route.clone();
    if route_id.is_empty() {
        return None;
    }
    let has = |h| app.harness_available(h);
    let named = settings
        .routes
        .iter()
        .find(|r| r.id == route_id && has(r.harness));
    if let Some(route) = named {
        return Some((route_id.clone(), Some(route.clone()), None));
    }
    let cheapest = cheapest_available(&settings, has);
    let note = cheapest.as_ref().filter(|_| route_id != "auto").map(|c| {
        format!(
            "route {route_id} is not configured or unavailable; using the cheapest available route {}",
            c.id
        )
    });
    Some((route_id, cheapest, note))
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
    let Some((route_id, route, fallback)) = check_route(app) else {
        return BriefAction::Proceed;
    };
    let (verdict, missing) = match &route {
        Some(route) => match run_check(app, &task, route, attempt_n, cancel).await {
            Ok(v) => v,
            Err(RunError::Cancelled) => return BriefAction::Cancelled,
            Err(RunError::Io(msg) | RunError::NotFound(msg)) => (Verdict::Unusable(msg), None),
        },
        None => (
            Verdict::Unusable(format!(
                "no route on an installed agent CLI for {route_id}; the check is off"
            )),
            None,
        ),
    };
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    let mut action = BriefAction::Proceed;
    if let Some(note) = fallback {
        task.decisions.push(format!("Brief check: {note}"));
    }
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
    // A redraft is checked again; anything else is settled for good. A tool
    // the task lacks is asked about before the first attempt, once.
    if !matches!(action, BriefAction::Redraft) && !task.brief_check.missing_tool_asked {
        if let Some(missing) = missing {
            task.decisions.push(format!(
                "Brief check: criterion {} needs {}, which the task will not have",
                missing.criterion, missing.capability
            ));
            task.brief_check.missing_tool = Some(missing);
        }
    }
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
        let b = build_check_brief("g", &["a".to_string()], &json!({"mcp": []}));
        assert!(b.starts_with("You check a coding task brief"));
        // Key order depends on serde_json's `preserve_order` (enabled by another workspace
        // crate): check each member, not the whole object.
        for part in [
            r#""criteria":["a"]"#,
            r#""goal":"g""#,
            r#""tools":{"mcp":[]}"#,
        ] {
            assert!(b.contains(part), "{part}");
        }
    }

    #[test]
    fn a_missing_tool_is_read_from_the_reply_and_checked_against_the_brief() {
        let off = vec![("asana".to_string(), "Asana".to_string())];
        let reply = r#"{"contradiction": false, "conflict": "", "missingTool": {"criterion": 1, "capability": " move an  Asana task ", "toolId": "asana"}}"#;
        let m = parse_missing_tool(reply, 2, &off).unwrap();
        assert_eq!(m.criterion, 1);
        assert_eq!(m.capability, "move an Asana task");
        assert_eq!(m.tool_id.as_deref(), Some("asana"));
        // An id that is not an off tool of this task is dropped, the claim kept.
        let other = reply.replace("\"asana\"}", "\"jira\"}");
        assert_eq!(parse_missing_tool(&other, 2, &off).unwrap().tool_id, None);
        // A criterion that does not exist, null and blank claims are nothing.
        assert!(parse_missing_tool(reply, 1, &off).is_none());
        assert!(parse_missing_tool(r#"{"missingTool": null}"#, 2, &off).is_none());
        assert!(parse_missing_tool(
            r#"{"missingTool": {"criterion": 0, "capability": " "}}"#,
            2,
            &off
        )
        .is_none());
    }
}
