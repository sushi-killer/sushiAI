use super::*;

const CHECK_PROMPT: &str = "You check a coding task brief before an agent works on it. Two checks.\n1. Decide whether any two requirements below (the goal and the acceptance criteria) contradict each other, so that satisfying one makes another impossible. Ordinary tension or extra work is not a contradiction.\n2. The agent will have only the capabilities listed under `tools`. Decide whether a criterion can only be met with a capability none of them provides, such as moving an Asana task when no Asana tool is listed. Work on files, shell commands and the repository needs no extra tool, and neither does a criterion that only asks the agent to report something.\nReply with only JSON: {\"contradiction\": true|false, \"conflict\": \"<which two requirements, quoted briefly, or empty>\", \"missingTool\": null or {\"criterion\": <0-based index of the criterion>, \"capability\": \"<what is missing, in a few words>\", \"toolId\": \"<id of a listed connected tool that is off for this task and would provide it, or empty>\"}}";

const REFRESH_PROMPT: &str = "A coding task was drafted before the tasks it builds on landed. Those tasks have landed and changed the files under `changedFiles`. Decide whether this task's `paths`, `verify` commands or `finalVerify` commands still name the right files and commands now that those files changed (a moved or renamed file, a new test file, a command whose target changed). Change only what is wrong and keep every other entry exactly as it is; never remove a check unless its target no longer exists, and never add a check that is not needed. Reply with only JSON: {\"verify\": [...], \"finalVerify\": [...], \"paths\": [...]}, each the complete list to use, or leave a field out to keep it.";

const MAX_CONFLICT_CHARS: usize = 400;

/// What the caller does after a check.
pub(super) enum BriefAction {
    Proceed,
    /// The planner's draft contradicts itself (the conflict text): draft
    /// again, once.
    Redraft(String),
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
    cancel: &CancelToken,
) -> Result<(Verdict, Option<MissingTool>, Option<String>), RunError> {
    let settings = app.settings.read().unwrap().clone();
    let tools = permissions::task_tools(app, &settings, task, Path::new(&task.worktree));
    let brief_text = build_check_brief(&task.goal, &task.criteria, &tools.capabilities());
    let JudgeRun {
        reply,
        cost,
        timeout_note,
    } = run_judge(
        app,
        task,
        route,
        None,
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
            (parse_verdict(&text), missing, timeout_note)
        }
        Err(why) => (Verdict::Unusable(why), None, timeout_note),
    })
}

pub(super) struct JudgeRun {
    pub reply: Result<String, String>,
    pub cost: f64,
    pub timeout_note: Option<String>,
}

/// One cheap read-only run of `route` on `prompt` in the task's worktree,
/// files in a claimed helper folder for `name`. Returns the reply (`Err` is a run that
/// ended in an error, `Ok` its final text), cost and optional timeout decision. The
/// caller adds them to its own copy of the task; only a cancelled run's partial
/// cost is added to the stored task here.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_judge(
    app: &Arc<App>,
    task: &Task,
    route: &Route,
    attempt_n: Option<u32>,
    stage: &'static str,
    name: &str,
    prompt: &str,
    cancel: &CancelToken,
) -> Result<JudgeRun, RunError> {
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, name, attempt_n)
        .map_err(|e| RunError::Io(e.to_string()))?;
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
        resume: None,
        ..review_request(route, &worktree, &mcp_path, &settings_path, &[])
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        &task.id,
        attempt_n.unwrap_or(0),
        RunTrack::No,
        &worktree,
        &req,
        if attempt_n.is_some() {
            CostTag::task(stage, &route.id)
        } else {
            CostTag::task_without_attempt(stage, &route.id)
        },
        prompt,
        &events_path,
        cancel,
        Guard::helper(name, limit),
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
    let timeout_note = outcome
        .timed_out
        .then(|| helper_timeout_message(name, limit));
    let cost = outcome.cost_usd.unwrap_or(0.0);
    if outcome.timed_out {
        return Ok(JudgeRun {
            reply: Err("the run timed out".to_string()),
            cost,
            timeout_note,
        });
    }
    if let Some(error) = outcome.error {
        return Ok(JudgeRun {
            reply: Err(format!("the run failed ({error})")),
            cost,
            timeout_note,
        });
    }
    Ok(JudgeRun {
        reply: Ok(outcome.final_text.unwrap_or_default()),
        cost,
        timeout_note,
    })
}

pub(super) fn add_task_cost(app: &Arc<App>, task_id: &str, cost: f64) {
    if cost <= 0.0 {
        return;
    }
    if let Err(e) = app.update_task(task_id, |task| task.cost_usd += cost) {
        eprintln!("orch: updating task {task_id} cost failed: {e}");
    }
}

/// The `briefCheckRoute` id, the route it resolves to (`None` when no route
/// on an available harness exists) and, when the id is not usable and the
/// cheapest available route stands in, the reason to record; `None` overall
/// when the setting is empty, i.e. cheap checks are off. `"auto"` names no
/// route: it is the cheapest configured route whose harness is available.
/// The variant may switch the check off or name another route.
pub(super) fn check_route_for(
    app: &App,
    variant: &Variant,
) -> Option<(String, Option<Route>, Option<String>)> {
    let settings = app.settings.read().unwrap();
    let route_id = variant.brief_check_route_id(&settings).to_string();
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

/// The line a fresh redraft appends to the request so the new planner
/// session knows what the previous draft got wrong.
pub(super) fn redraft_request_note(conflict: &str) -> String {
    format!(
        "(Brief check: the previous draft's requirements contradict each other: {conflict}. Draft again so that no two requirements conflict.)"
    )
}

/// Checks the stored task's goal and criteria once. `can_redraft` is true
/// for a planner draft: a contradiction then asks the planner to correct it
/// (at most once per task); the caller changes the request only when it has
/// to draft from scratch (see [`redraft_request_note`]). Otherwise, and for an unusable reply, the task
/// carries on; a conflict is kept for the implement and review briefs.
pub(super) async fn check_brief(
    app: &Arc<App>,
    task_id: &str,
    can_redraft: bool,
    cancel: &CancelToken,
) -> BriefAction {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    if task.brief_check.done {
        return BriefAction::Proceed;
    }
    let Some((route_id, route, fallback)) = check_route_for(app, &task.variant()) else {
        return BriefAction::Proceed;
    };
    let (verdict, missing, timeout_note) = match &route {
        Some(route) => match run_check(app, &task, route, cancel).await {
            Ok(v) => v,
            Err(RunError::Cancelled) => return BriefAction::Cancelled,
            Err(RunError::Io(msg) | RunError::NotFound(msg)) => {
                (Verdict::Unusable(msg), None, None)
            }
        },
        None => (
            Verdict::Unusable(format!(
                "no route on an installed agent CLI for {route_id}; the check is off"
            )),
            None,
            None,
        ),
    };
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    task.decisions.extend(timeout_note);
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
                task.status = TaskStatus::Drafting;
                action = BriefAction::Redraft(conflict);
            } else {
                task.decisions.push(format!("Brief check: {conflict}"));
                task.brief_check.conflict = Some(conflict);
            }
        }
    }
    // A redraft is checked again; anything else is settled for good. A tool
    // the task lacks is asked about before the first attempt, once.
    if !matches!(action, BriefAction::Redraft(_)) && !task.brief_check.missing_tool_asked {
        if let Some(missing) = missing {
            task.decisions.push(format!(
                "Brief check: criterion {} needs {}, which the task will not have",
                missing.criterion, missing.capability
            ));
            task.brief_check.missing_tool = Some(missing);
        }
    }
    task.brief_check.done = !matches!(action, BriefAction::Redraft(_));
    task.updated_at = now_ms();
    app.save_task_logged(&task);
    app.broadcast_task(&task);
    action
}

/// The files the dependencies changed: each one's landed diff (or, not yet
/// landed, its branch over its base) read from git; the last passed implement
/// attempt's changed files when git cannot read the range.
async fn landed_files(app: &App, task: &Task) -> Vec<String> {
    let deps: Vec<Task> = task
        .depends_on
        .iter()
        .filter_map(|id| app.store.load_task(id).ok().flatten())
        .collect();
    let mut files: Vec<String> = Vec::new();
    for dep in deps {
        let (from, to) = super::reports::task_range(&dep);
        let repo = PathBuf::from(&dep.repo);
        let from_git = tokio::task::spawn_blocking(move || git::numstat(&repo, &from, &to).ok())
            .await
            .unwrap_or(None);
        let paths: Vec<String> = match from_git {
            Some(entries) => entries.into_iter().map(|e| e.0).collect(),
            None => dep
                .attempts
                .iter()
                .rev()
                .find(|a| a.stage == Stage::Implement && a.status == AttemptStatus::Passed)
                .map(|a| a.changed_files.clone())
                .unwrap_or_default(),
        };
        for f in paths {
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    files
}

/// A string-array member of the reply; `None` when it is absent or not a list
/// of strings.
fn reply_list(v: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    v.get(key)?
        .as_array()?
        .iter()
        .map(|x| x.as_str().map(|s| s.trim().to_string()))
        .collect::<Option<Vec<_>>>()
        .map(|l| l.into_iter().filter(|s| !s.is_empty()).collect())
}

/// Once per parent-drafted child, after its dependencies landed and before
/// its first attempt: when what they changed touches the child's `paths`, or
/// a file its verify/finalVerify commands name, one cheap judge run on the
/// brief-check route may correct `verify`, `finalVerify` and `paths`. Never
/// the title, goal, criteria or tier, and never a planner run. A rewrite that
/// removes a shared check is refused. Returns `true` when the task was
/// cancelled while the run was live (the flag is then not set).
pub(super) async fn refresh_after_dependencies(
    app: &Arc<App>,
    task_id: &str,
    cancel: &CancelToken,
) -> bool {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return false;
    };
    if !task.planned_by_parent
        || task.depends_on.is_empty()
        || task.brief_check.refreshed
        || completed_implement_attempts(&task) > 0
    {
        return false;
    }
    let changed = landed_files(app, &task).await;
    let commands: Vec<&String> = task.verify.iter().chain(&task.final_verify).collect();
    let targets: Vec<String> = commands.iter().flat_map(|c| check_targets(c)).collect();
    let touched = overlapping_path(&task.paths, &changed).is_some()
        || changed.iter().any(|f| targets.iter().any(|t| t == f));
    let settle = |decision: Option<String>, edit: &dyn Fn(&mut Task)| {
        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            edit(&mut t);
            t.brief_check.refreshed = true;
            t.decisions.extend(decision);
            t.updated_at = now_ms();
            app.save_task_logged(&t);
            app.broadcast_task(&t);
        }
    };
    if !touched {
        settle(None, &|_| {});
        return false;
    }
    let Some((route_id, route, fallback)) = check_route_for(app, &task.variant()) else {
        settle(
            Some("Refresh: skipped, the brief-check route is empty".to_string()),
            &|_| {},
        );
        return false;
    };
    let Some(route) = route else {
        settle(
            Some(format!(
                "Refresh: skipped, no route on an installed agent CLI for {route_id}"
            )),
            &|_| {},
        );
        return false;
    };
    let prompt = format!(
        "{REFRESH_PROMPT}\n\n{}\n",
        json!({
            "changedFiles": changed,
            "paths": task.paths,
            "verify": task.verify,
            "finalVerify": task.final_verify,
        })
    );
    let (reply, timeout_note) = match run_judge(
        app, &task, &route, None, "refresh", "refresh", &prompt, cancel,
    )
    .await
    {
        Ok(JudgeRun {
            reply,
            cost,
            timeout_note,
        }) => {
            add_task_cost(app, task_id, cost);
            (reply, timeout_note)
        }
        Err(RunError::Cancelled) => return true,
        Err(RunError::Io(msg) | RunError::NotFound(msg)) => (Err(msg), None),
    };
    if let Some(line) = timeout_note {
        settle(Some(line), &|_| {});
    }
    let note = fallback.map(|n| format!("Refresh: {n}"));
    let parsed = reply.and_then(|text| {
        let (start, end) = (text.find('{'), text.rfind('}'));
        let slice = start.zip(end).filter(|(s, e)| s < e);
        let (s, e) = slice.ok_or_else(|| "the reply had no JSON".to_string())?;
        serde_json::from_str::<serde_json::Value>(&text[s..=e])
            .map_err(|_| "the reply was not valid JSON".to_string())
    });
    let v = match parsed {
        Ok(v) => v,
        Err(why) => {
            let mut lines: Vec<String> = note.into_iter().collect();
            lines.push(format!(
                "Refresh: no usable answer ({}); kept as drafted",
                truncate_chars(&why, 200)
            ));
            settle(Some(lines.join("\n")), &|_| {});
            return false;
        }
    };
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return false;
    };
    // A rewrite that empties a list the draft had is a removal, not a fix.
    let differs = |new: Option<Vec<String>>, old: &[String]| {
        new.filter(|n| *n != old && (old.is_empty() || !n.is_empty()))
    };
    let verify = differs(reply_list(&v, "verify"), &task.verify);
    let final_verify = differs(reply_list(&v, "finalVerify"), &task.final_verify);
    let paths = differs(reply_list(&v, "paths"), &task.paths);
    let amendment = Amendment {
        verify: verify.clone(),
        final_verify: final_verify.clone(),
        ..Default::default()
    };
    let settings = app.settings.read().unwrap().clone();
    let refused = (!amendment.is_empty())
        .then(|| shared_check_amend_refusal(&settings, &task, &amendment))
        .flatten();
    let (verify, final_verify) = if refused.is_some() {
        (None, None)
    } else {
        (verify, final_verify)
    };
    let mut fields: Vec<&str> = Vec::new();
    if verify.is_some() {
        fields.push("verify");
    }
    if final_verify.is_some() {
        fields.push("finalVerify");
    }
    if paths.is_some() {
        fields.push("paths");
    }
    let mut decision = match (&refused, fields.is_empty()) {
        (Some(why), _) => format!("Refresh: refused, kept the old commands ({why})"),
        (None, true) => "Refresh: no change".to_string(),
        (None, false) => format!("Refresh: changed {}", fields.join(", ")),
    };
    if refused.is_some() && !fields.is_empty() {
        decision.push_str(&format!("; changed {}", fields.join(", ")));
    }
    if verify.is_some() {
        // Cached results belong to the old commands.
        app.verify_cache.lock().unwrap().remove(task_id);
    }
    let lines: Vec<String> = note.into_iter().chain([decision]).collect();
    settle(Some(lines.join("\n")), &|t| {
        if let Some(v) = verify.clone() {
            t.verify = v;
        }
        if let Some(v) = final_verify.clone() {
            t.final_verify = v;
        }
        if let Some(p) = paths.clone() {
            t.paths = p;
        }
    });
    false
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

    #[tokio::test]
    async fn a_split_parent_dependency_triggers_the_refresh_from_its_landed_diff() {
        use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
        let (app, dir) = test_app();
        app.settings.write().unwrap().brief_check_route = String::new();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        init_git_repo(&repo);
        std::fs::write(repo.join("src/touched.txt"), "x\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "landed"]);
        let landed = git_in(&repo, &["rev-parse", "HEAD"]);
        // A split parent: no implement attempt of its own, only a landing.
        let mut dep = task_with_status(TaskStatus::Done);
        dep.repo = repo.display().to_string();
        dep.landed_sha = Some(landed);
        app.store.save_task(&dep).unwrap();

        let child_with = |paths: &[&str]| {
            let mut t = task_with_status(TaskStatus::Queued);
            t.repo = repo.display().to_string();
            t.planned_by_parent = true;
            t.depends_on = vec![dep.id.clone()];
            t.paths = paths.iter().map(|p| p.to_string()).collect();
            app.store.save_task(&t).unwrap();
            t
        };
        let hit = child_with(&["src/touched.txt"]);
        let miss = child_with(&["src/other.txt"]);

        for t in [&hit, &miss] {
            let cancelled = refresh_after_dependencies(&app, &t.id, &CancelToken::new()).await;
            assert!(!cancelled);
        }
        let refresh_lines = |t: &Task| {
            app.store
                .load_task(&t.id)
                .unwrap()
                .unwrap()
                .decisions
                .iter()
                .filter(|d| d.starts_with("Refresh:"))
                .count()
        };
        assert_eq!(
            refresh_lines(&hit),
            1,
            "the touched part starts the refresh"
        );
        assert_eq!(refresh_lines(&miss), 0);
        assert!(
            app.store
                .load_task(&miss.id)
                .unwrap()
                .unwrap()
                .brief_check
                .refreshed
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn brief_check_timed_out_continues_without_a_usable_answer() {
        let (app, _dir, task, _) = slow_helper_task();
        assert!(matches!(
            check_brief(&app, &task.id, false, &CancelToken::new()).await,
            BriefAction::Proceed
        ));
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(task.brief_check.done);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.starts_with("Brief check: no usable answer") && d.ends_with("continuing")));
        assert_helper_timeout(&task, "brief-check");
        let cost = crate::costs::read_all(&app.data_dir);
        assert_eq!(cost.len(), 1);
        assert!(cost[0].attempt.is_none());
    }
}
