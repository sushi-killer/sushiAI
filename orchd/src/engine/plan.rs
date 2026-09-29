use super::*;

pub(super) const MAX_SUBTASKS: usize = 8;

/// Whether the planner's split can become a graph: at least two parts,
/// unique non-empty keys, a request each, dependencies on known keys only,
/// and no cycle.
pub(super) fn check_subtasks(subtasks: &[brief::PlanSubtask]) -> Result<(), String> {
    if subtasks.len() < 2 {
        return Err("a single part is the task itself".to_string());
    }
    if subtasks.len() > MAX_SUBTASKS {
        return Err(format!("more than {MAX_SUBTASKS} parts"));
    }
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for s in subtasks {
        let key = s.key.trim();
        if key.is_empty() || s.request.trim().is_empty() {
            return Err("a part without a key or a request".to_string());
        }
        if edges.contains_key(key) {
            return Err(format!("duplicate key {key}"));
        }
        edges.insert(key.to_string(), s.depends_on.clone());
    }
    for (key, deps) in &edges {
        if let Some(unknown) = deps.iter().find(|d| !edges.contains_key(d.trim())) {
            return Err(format!("{key} depends on unknown key {unknown}"));
        }
    }
    if has_cycle(&edges) {
        return Err("the parts depend on each other in a cycle".to_string());
    }
    Ok(())
}

fn path_components(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// The first pair of paths where one equals or is a directory prefix of the
/// other, compared per path component: returns the shorter (the shared area).
pub(super) fn overlapping_path<'a>(a: &'a [String], b: &'a [String]) -> Option<&'a str> {
    for pa in a {
        let ca = path_components(pa);
        for pb in b {
            let cb = path_components(pb);
            let n = ca.len().min(cb.len());
            if ca[..n] == cb[..n] {
                return Some(if ca.len() <= cb.len() { pa } else { pb });
            }
        }
    }
    None
}

/// Whether `from` waits (directly or transitively) for `target`.
fn waits_for(deps: &HashMap<String, Vec<String>>, from: &str, target: &str) -> bool {
    let mut stack = vec![from.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(node) = stack.pop() {
        for d in deps.get(&node).into_iter().flatten() {
            if d == target {
                return true;
            }
            if seen.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    false
}

/// Siblings that touch the same paths would conflict at landing, so the later
/// one in the list also waits for the earlier one unless they are already
/// ordered. A part without paths overlaps every sibling. Edges only go from
/// earlier to later positions, so no cycle can result. Returns the parts with
/// the added `dependsOn` keys and one decision line per added edge.
pub(super) fn serialise_overlapping(
    parts: &[brief::PlanSubtask],
) -> (Vec<brief::PlanSubtask>, Vec<String>) {
    let mut out = parts.to_vec();
    let mut deps: HashMap<String, Vec<String>> = out
        .iter()
        .map(|p| {
            (
                p.key.trim().to_string(),
                p.depends_on.iter().map(|d| d.trim().to_string()).collect(),
            )
        })
        .collect();
    let mut decisions = Vec::new();
    for j in 1..out.len() {
        for i in 0..j {
            let (a, b) = (out[i].key.trim().to_string(), out[j].key.trim().to_string());
            if waits_for(&deps, &b, &a) || waits_for(&deps, &a, &b) {
                continue;
            }
            let shared: Option<String> = if out[i].paths.is_empty() || out[j].paths.is_empty() {
                Some("(no paths declared)".to_string())
            } else {
                overlapping_path(&out[i].paths, &out[j].paths).map(str::to_string)
            };
            let Some(shared) = shared else { continue };
            deps.get_mut(&b).unwrap().push(a.clone());
            out[j].depends_on.push(a.clone());
            decisions.push(format!("Serialised {b} after {a}: both touch {shared}"));
        }
    }
    (out, decisions)
}

/// Which earlier sibling each part continues as a relay, and one decision
/// line per link. Parts whose declared paths overlap and were serialised
/// run as one branch: a part continues the previous overlapping
/// part's commit instead of branching from the parent, so it edits on top of
/// that work and only the last part lands on the parent. A part only joins a
/// chain when every earlier part it overlaps and every part it depends on is
/// already in that chain and nothing continues the same predecessor;
/// otherwise it stays an ordinary serialised sibling. `parts` gain the
/// `dependsOn` a link needs, and a part that depends on a chain's middle
/// link also waits for the chain's last one, which is what lands.
pub(super) fn relay_links(parts: &mut [brief::PlanSubtask]) -> (Vec<Option<usize>>, Vec<String>) {
    let keys: Vec<String> = parts.iter().map(|p| p.key.trim().to_string()).collect();
    // Only parts that both declare paths can be linked: a part without paths
    // is serialised, and its work lands on the parent like any sibling's.
    let overlaps = |i: usize, j: usize| {
        !parts[i].paths.is_empty()
            && !parts[j].paths.is_empty()
            && overlapping_path(&parts[i].paths, &parts[j].paths).is_some()
    };
    let mut link: Vec<Option<usize>> = vec![None; parts.len()];
    let mut has_next = vec![false; parts.len()];
    let chain_of = |link: &[Option<usize>], mut at: usize| {
        let mut chain = vec![at];
        while let Some(prev) = link[at] {
            chain.push(prev);
            at = prev;
        }
        chain
    };
    for j in 1..parts.len() {
        let earlier: Vec<usize> = (0..j).filter(|&i| overlaps(i, j)).collect();
        let Some(&p) = earlier.last() else { continue };
        if has_next[p] {
            continue;
        }
        let chain = chain_of(&link, p);
        let deps_in_chain = parts[j].depends_on.iter().all(|d| {
            keys.iter()
                .position(|k| k == d.trim())
                .is_some_and(|at| chain.contains(&at))
        });
        if earlier.iter().all(|i| chain.contains(i)) && deps_in_chain {
            link[j] = Some(p);
            has_next[p] = true;
        }
    }
    let mut decisions = Vec::new();
    for j in 0..parts.len() {
        let Some(p) = link[j] else { continue };
        if !parts[j].depends_on.iter().any(|d| d.trim() == keys[p]) {
            parts[j].depends_on.push(keys[p].clone());
        }
        decisions.push(format!(
            "Relay: {} continues on {}'s branch, one chain landed on the parent once",
            keys[j], keys[p]
        ));
    }
    // A part outside a chain that builds on one of its middle links needs
    // the chain's last link: that is the one that lands the work.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for i in 0..parts.len() {
        if link[i].is_some() {
            continue;
        }
        let mut extra = Vec::new();
        for d in parts[i].depends_on.clone() {
            let Some(mut at) = keys.iter().position(|k| k == d.trim()) else {
                continue;
            };
            let mut moved = false;
            while let Some(next) = (0..parts.len()).find(|&n| link[n] == Some(at)) {
                at = next;
                moved = true;
            }
            if moved {
                extra.push(keys[at].clone());
            }
        }
        for key in extra {
            if !parts[i].depends_on.iter().any(|d| d.trim() == key) {
                parts[i].depends_on.push(key);
            }
        }
    }
    for part in parts.iter() {
        edges.insert(
            part.key.trim().to_string(),
            part.depends_on
                .iter()
                .map(|d| d.trim().to_string())
                .collect(),
        );
    }
    if has_cycle(&edges) {
        return (vec![None; parts.len()], Vec::new());
    }
    (link, decisions)
}

/// A subtask's request: the planner's text first (what its own planner
/// drafts from), then the whole it belongs to and what was already decided
/// for it.
pub(super) fn subtask_request(parent: &Task, part: &str) -> String {
    let mut out = format!(
        "{}\n\nThis is one part of a larger task, \"{}\": {}",
        part.trim(),
        parent.title,
        parent.goal
    );
    let decided: Vec<&String> = parent
        .decisions
        .iter()
        .filter(|d| d.starts_with("Owner:") || d.starts_with("Orchestrator:"))
        .collect();
    if !decided.is_empty() {
        out.push_str("\n\nAlready decided for the whole task:\n");
        for d in decided {
            out.push_str(&format!("- {d}\n"));
        }
    }
    out
}

/// The one owner question for a plan's blocking questions: each with its
/// options and the planner's recommendation, answered in one free-text reply.
fn batched_question_text(questions: &[&brief::PlanQuestion]) -> String {
    let mut out = format!(
        "The planner has {} blocking question(s); answer them all in one reply:\n",
        questions.len()
    );
    for (i, q) in questions.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", i + 1, q.text));
        if !q.options.is_empty() {
            out.push_str(&format!(" (options: {})", q.options.join(" / ")));
        }
        if let Some(r) = q.usable_recommendation() {
            out.push_str(&format!(" -- recommended: {r}"));
        }
        if !q.evidence.trim().is_empty() {
            out.push_str(&format!(" ({})", q.evidence.trim()));
        }
    }
    out
}

/// Sets the question/waiting state, parks on a fresh one-shot the same way
/// [`wait_for_answer`] does, then records the decision as `"Owner: <question>
/// -> <answer>"` (spec step 5) instead of the generic attempt-failure
/// phrasing. `"stop"` is still handled centrally by `handle_task_answer`
/// before it ever reaches here.
pub(super) async fn ask_plan_question(
    app: &Arc<App>,
    task_id: &str,
    question_text: &str,
    options: Vec<String>,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    // Same as wait_for_answer: a task parked on a question holds no slot.
    permit.take();
    // Install the one-shot *before* the state that makes this externally
    // visible as "waiting" is even persisted: otherwise a `task.answer`
    // that arrives the instant the broadcast reaches a client could beat
    // this into existence and get rejected as "no live parked loop" even
    // though a loop genuinely is about to park.
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        task.question = Some(Question {
            text: question_text.to_string(),
            options,
            kind: QuestionKind::PlanQuestion,
        });
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
    }

    let result = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;

    match &result {
        Some(answer) => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                task.decisions
                    .push(format!("Owner: {question_text} -> {answer}"));
                task.question = None;
                task.status = TaskStatus::Drafting;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                let next = app.cancelled_status(&task.status);
                if task.status != next {
                    task.status = next;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
            }
        }
    }
    result
}

pub enum PlanOutcome {
    /// Planning finished and `auto_start_after_plan` was set: the task is
    /// now `queued`, fall straight through to the implement stage.
    Proceed,
    /// Planning finished but `auto_start_after_plan` was not set: the task
    /// is `stopped` with everything filled in for the owner to review.
    StoppedForReview,
    /// Cancelled, or ended in `failed`/`stopped` some other way -- the task
    /// loop itself is done, the caller should return.
    Ended,
}

/// Whether a task still needs to run (or re-run) the plan stage, derived
/// purely from data -- never from `task.status` -- so a stop-then-start
/// while drafting, an owner answer that arrives with no live loop to catch
/// it (daemon restart), or a manual `task.start` on a task recovery left
/// `stopped`/`failed` mid-draft, all correctly go back through planning
/// instead of quietly falling through to implement with blank fields. Once
/// one plan attempt has *passed*, this is permanently `false` for the rest
/// of the task's life -- planning runs at most once, ever.
pub(super) fn needs_planning(task: &Task) -> bool {
    task.request.is_some()
        && !task
            .attempts
            .iter()
            .any(|a| a.stage == Stage::Plan && a.status == AttemptStatus::Passed)
}

/// Common cleanup before returning `PlanOutcome::Ended`: if the plan
/// attempt at `idx` is still `running` (cut short by cancellation between
/// its own steps, rather than already concluded `passed`/`failed` by the
/// caller), mark it `interrupted`; either way, make sure the task itself
/// ends up `stopped` unless it's already some other terminal status.
async fn end_plan_stage(app: &Arc<App>, task_id: &str, idx: Option<usize>) -> PlanOutcome {
    if let Some(idx) = idx {
        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            if let Some(a) = t.attempts.get_mut(idx) {
                if a.status == AttemptStatus::Running {
                    a.status = AttemptStatus::Interrupted;
                    a.ended_at = Some(now_ms());
                    let run_dir = app.store.run_dir(task_id, a.n);
                    let prices = app.settings.read().unwrap().prices.clone();
                    settle_unfinished_cost(&mut t, idx, &run_dir, &prices);
                    t.updated_at = now_ms();
                    let _ = app.store.save_task(&t);
                    app.broadcast_task(&t);
                }
            }
        }
    }
    mark_stopped_if_not_already(app, task_id).await;
    PlanOutcome::Ended
}

/// Runs the drafting stage (spec "Plan stage" steps 1-6) for a task created
/// via the `{repo, request}` form: a fresh read-only planner session drafts
/// title/goal/criteria/verify from the owner's one-sentence request (one
/// retry if the reply doesn't parse, then the owner is asked directly, up
/// to 2 such clarification rounds before giving up), and asks any of the
/// planner's own questions
/// (plus a verification question whenever the draft still has none) one at
/// a time before handing off to the ordinary implement loop.
pub(super) async fn run_plan_stage(
    app: &Arc<App>,
    task_id: &str,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
    auto_start_after_plan: bool,
) -> PlanOutcome {
    let mut clarify_rounds: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            return end_plan_stage(app, task_id, None).await;
        }
        // The very first round already holds the slot `run_task_loop`
        // acquired before calling in; a round that starts fresh right after
        // releasing one to park on a clarifying question needs its own, or
        // it would run the next harness session with no permit at all.
        if permit.is_none() {
            let acquired = tokio::select! {
                _ = cancel.cancelled() => None,
                p = app.slots.clone().acquire_owned() => p.ok(),
            };
            match acquired {
                Some(p) => *permit = Some(p),
                None => return end_plan_stage(app, task_id, None).await,
            }
        }

        let mut task = match app.store.load_task(task_id) {
            Ok(Some(t)) => t,
            _ => return end_plan_stage(app, task_id, None).await,
        };
        if !wait_while_over_budget(
            app,
            task_id,
            &mut task,
            "plan",
            TaskStatus::Drafting,
            pending_answer,
            cancel,
            permit,
        )
        .await
        {
            return end_plan_stage(app, task_id, None).await;
        }

        let settings = app.settings.read().unwrap().clone();
        let variant = task.variant();
        let planner = variant.plan_route_id(&settings);
        let Some(route) = settings.routes.iter().find(|r| r.id == planner).cloned() else {
            // An unconfigured planner id is treated exactly like planner ==
            // "" -- never a silent fallback to some route the owner never
            // chose for this.
            if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                t.status = TaskStatus::Failed;
                t.updated_at = now_ms();
                let _ = app.store.save_task(&t);
                app.broadcast_task(&t);
            }
            return end_plan_stage(app, task_id, None).await;
        };

        if variant.planner_route.is_some() {
            let line = variant_route_line("planner", &route.id);
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }

        let request_text = task.request.clone().unwrap_or_default();
        let attempt_n = task.attempts.len() as u32 + 1;
        let worktree = PathBuf::from(&task.worktree);
        let run_dir = app.store.run_dir(task_id, attempt_n).join("plan");
        let _ = std::fs::create_dir_all(&run_dir);

        let attempt = Attempt {
            n: attempt_n,
            stage: Stage::Plan,
            route_id: route.id.clone(),
            harness: route.harness,
            model: route.model.clone().unwrap_or_default(),
            reason: "drafting".to_string(),
            session_id: None,
            pgid: None,
            started_at: now_ms(),
            ended_at: None,
            status: AttemptStatus::Running,
            summary: None,
            handoff: None,
            changed_files: vec![],
            verify: vec![],
            gate_blocks: 0,
            prefix_tokens: None,
            review: None,
            failure: None,
            usage: None,
            cost_usd: None,
            cost_estimated: false,
            review_cost_usd: None,
            evidence: vec![],
            advice: None,
            advisor_cost_usd: None,
            fingerprint: None,
            review_fingerprint: None,
            advisor_fingerprint: None,
            candidates: vec![],
        };
        task.attempts.push(attempt);
        let idx = task.attempts.len() - 1;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        let mcp_path = run_dir.join("mcp.json");
        let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
        let settings_path = run_dir.join("settings.json");
        let key_path = run_dir.join("key");
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        if matches!(route.harness, Harness::Claude) {
            // Same lean shape as review (read-only, no Stop hook -- no
            // token is ever registered for a drafting session), but the
            // planner route can still carry a profile (env/API key) the
            // same way an implement route can.
            write_readonly_claude_settings_with_profile(
                &route,
                app,
                &key_path,
                &settings,
                &deny_read,
                &settings_path,
            );
        }
        let req = harness::RunRequest {
            harness: route.harness,
            worktree: &worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            max_budget_usd: None,
            review: true,
            mcp_config: Some(&mcp_path),
            settings_path: Some(&settings_path),
            network_allowed: false,
            codex_mcp: None,
            images: &[],
            repo_settings: true,
        };

        // Once per round: the retry brief carries the same section.
        let past_work = {
            let all_tasks = app.store.list_tasks().unwrap_or_default();
            let notes = app
                .store
                .load_repo_notes()
                .unwrap_or_default()
                .remove(&task.repo)
                .unwrap_or_default();
            super::past_work::past_work_section(&task, &notes, &all_tasks)
        };
        let mut draft: Option<brief::PlanDraft> = None;
        let mut hard_failure = false;
        let mut over_budget = false;
        for retry in 0..2 {
            if cancel.is_cancelled() {
                let _ = std::fs::remove_file(&key_path);
                return end_plan_stage(app, task_id, Some(idx)).await;
            }
            if retry > 0 {
                // The retry is a run of its own: when the first one reached
                // the budget, this attempt ends and the next round's gate
                // asks the owner before any further planner run.
                if task.cost_budget().is_some_and(|b| task.cost_usd >= b) {
                    over_budget = true;
                    break;
                }
                // The first run's cost, saved before the reload after the
                // retry would drop it; recovery then counts only the retry.
                let _ = app.store.save_task(&task);
            }
            let brief_text = match (retry == 0, task.parent.is_some()) {
                (true, false) => {
                    brief::build_plan_brief(&request_text, &task.variant(), &past_work)
                }
                (false, false) => {
                    brief::build_plan_retry_brief(&request_text, &task.variant(), &past_work)
                }
                (true, true) => {
                    brief::build_subtask_plan_brief(&request_text, &task.variant(), &past_work)
                }
                (false, true) => brief::build_subtask_plan_retry_brief(
                    &request_text,
                    &task.variant(),
                    &past_work,
                ),
            };
            let brief_text = brief::with_block_before_report(
                &brief_text,
                &brief::landed_dependencies_block(&task, &app.repo_tasks(&task.repo), None),
            );
            let file_stem = if retry == 0 { "" } else { "-retry" };
            let _ = std::fs::write(run_dir.join(format!("brief{file_stem}.md")), &brief_text);
            let events_path = run_dir.join(format!("events{file_stem}.jsonl"));
            let run_result = run_harness(
                app,
                task_id,
                attempt_n,
                true,
                &worktree,
                &req,
                CostTag::task("plan", &route.id),
                &brief_text,
                &events_path,
                cancel,
                None,
                None,
            )
            .await;
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            match run_result {
                Ok(o) => {
                    if let Some(cost) = o.cost_usd {
                        let a = &mut task.attempts[idx];
                        a.cost_usd = Some(a.cost_usd.unwrap_or(0.0) + cost);
                        a.cost_estimated |= o.cost_estimated;
                        task.cost_usd += cost;
                    }
                    task.attempts[idx].fingerprint = o.fingerprint.clone();
                    let usage = task.attempts[idx].usage.get_or_insert(Usage {
                        input: 0,
                        output: 0,
                        cached: 0,
                    });
                    usage.input += o.usage_input;
                    usage.output += o.usage_output;
                    usage.cached += o.usage_cached;
                    // A harness-reported failure (Claude `is_error`, Codex
                    // `turn.failed`) is an infra/tooling problem, not an
                    // ambiguous-request problem -- fail outright rather than
                    // spend a clarification round asking the owner "more
                    // detail" about something they can't fix by replying.
                    if let Some(err) = o.error {
                        record_failure(&mut task, idx, FailureKind::Error, err);
                        task.status = TaskStatus::Failed;
                        task.updated_at = now_ms();
                        let _ = app.store.save_task(&task);
                        app.broadcast_task(&task);
                        hard_failure = true;
                        break;
                    }
                    let text = o.final_text.unwrap_or_default();
                    if let Some(d) = brief::parse_plan(&text) {
                        draft = Some(d);
                        break;
                    }
                }
                Err(RunError::Cancelled) => {
                    let _ = std::fs::remove_file(&key_path);
                    return end_plan_stage(app, task_id, Some(idx)).await;
                }
                Err(RunError::Io(msg)) => {
                    record_failure(&mut task, idx, FailureKind::Error, msg);
                    task.status = TaskStatus::Failed;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    hard_failure = true;
                    break;
                }
            }
        }
        // Delete the per-run key file the moment the run(s) end, same as
        // the implement path; recovery also deletes it for an attempt
        // interrupted by an unclean shutdown.
        let _ = std::fs::remove_file(&key_path);
        if hard_failure {
            return end_plan_stage(app, task_id, Some(idx)).await;
        }
        if over_budget {
            // Not a clarification round: the owner is asked about the
            // budget, not for more detail, and "raise" drafts again.
            record_failure(
                &mut task,
                idx,
                FailureKind::NoDeliverable,
                "planner did not return a parseable sushi-plan block; the budget stopped the retry"
                    .to_string(),
            );
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            continue;
        }

        let Some(draft) = draft else {
            record_failure(
                &mut task,
                idx,
                FailureKind::NoDeliverable,
                "planner did not return a parseable sushi-plan block".to_string(),
            );
            if clarify_rounds >= 2 {
                // Already asked the owner twice for more detail; a third
                // unparseable round in a row means this isn't going anywhere.
                task.status = TaskStatus::Failed;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                return end_plan_stage(app, task_id, Some(idx)).await;
            }
            clarify_rounds += 1;
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            let question =
                "The planner could not draft this task: edit it or answer with more detail";
            match ask_plan_question(
                app,
                task_id,
                question,
                vec![],
                pending_answer,
                cancel,
                permit,
            )
            .await
            {
                Some(answer) => {
                    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                        let combined = format!(
                            "{}\n\n(Owner clarification: {answer})",
                            t.request.clone().unwrap_or_default()
                        );
                        t.request = Some(combined);
                        t.status = TaskStatus::Drafting;
                        t.updated_at = now_ms();
                        let _ = app.store.save_task(&t);
                        app.broadcast_task(&t);
                    }
                    continue;
                }
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        };

        task.title = draft.title.clone();
        task.goal = draft.goal.clone();
        task.planned_tier = draft.tier;
        // With review off nobody would check a moved entry: keep it as is.
        let (verify, judged) = if settings.review.is_empty() {
            (draft.verify.clone(), Vec::new())
        } else {
            split_verify_commands(&worktree, &draft.verify)
        };
        // What the planner says the task edits: the queue holds it back while
        // another live task on the same base holds any of these paths.
        if task.paths.is_empty() {
            task.paths = draft
                .paths
                .iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
        }
        task.criteria = draft.criteria.clone();
        task.visual_criteria = draft.visual_criteria.clone();
        task.criteria.extend(
            judged
                .into_iter()
                .map(|v| format!("Checked by review (not a shell command): {v}")),
        );
        task.verify = verify;
        if task.variant().grounded_checks {
            // Stored before any split, so a splitting parent keeps them.
            task.checks = valid_checks(draft.checks.clone(), draft.criteria.len());
            task.held_out = valid_check(draft.held_out.clone(), draft.criteria.len());
        }
        task.final_verify = draft
            .final_verify
            .iter()
            .filter(|c| !c.trim().is_empty())
            .cloned()
            .collect();
        task.attempts[idx].status = AttemptStatus::Passed;
        task.attempts[idx].ended_at = Some(now_ms());
        task.attempts[idx].summary = Some(format!("Drafted: {}", draft.title));
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        // Do the draft's own requirements contradict each other? One
        // redraft, once per task; the check runs again on the new draft.
        match check_brief(app, task_id, attempt_n, true, cancel).await {
            BriefAction::Redraft => continue,
            BriefAction::Cancelled => return end_plan_stage(app, task_id, Some(idx)).await,
            BriefAction::Proceed => {}
        }
        if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
            task = reloaded;
        }

        let mut asked: Vec<&brief::PlanQuestion> = draft.questions.iter().take(3).collect();
        if task.variant().batch_questions {
            // Non-blocking questions with a usable recommendation are not
            // asked: each becomes an assumption and planning goes on.
            let (assumed, blocking): (Vec<_>, Vec<_>) = asked
                .into_iter()
                .partition(|q| !q.blocking && q.usable_recommendation().is_some());
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            for q in assumed {
                // Planning reruns until a plan attempt passes, so the same
                // question can come back after an interruption: record it once.
                if task.assumptions.iter().any(|a| a.question == q.text) {
                    continue;
                }
                let answer = q.usable_recommendation().unwrap_or_default().to_string();
                app.broadcast_log(
                    task_id,
                    attempt_n,
                    format!("Planner assumed: {} -> {answer}", q.text),
                );
                task.assumptions.push(Assumption {
                    question: q.text.clone(),
                    answer,
                    evidence: q.evidence.clone(),
                    by: "planner".to_string(),
                    kind: None,
                    attempt: None,
                    overturned: false,
                    owner_answer: None,
                });
            }
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            asked = Vec::new();
            if !blocking.is_empty() {
                // One owner question for all of them, one free-text answer.
                let text = batched_question_text(&blocking);
                if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                    task = reloaded;
                }
                match ask_plan_question_with_triage(
                    app,
                    task_id,
                    &mut task,
                    attempt_n,
                    &worktree,
                    &text,
                    vec![],
                    true,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    Some(_) => {}
                    None => return end_plan_stage(app, task_id, Some(idx)).await,
                }
            }
        }
        for q in asked {
            // P2: reload before each question -- an earlier question in
            // this same loop may have gone to the owner and back (or been
            // triage-answered) since `task` was last captured, and triage's
            // brief for this one must see that, not a stale snapshot.
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            match ask_plan_question_with_triage(
                app,
                task_id,
                &mut task,
                attempt_n,
                &worktree,
                &q.text,
                q.options.clone(),
                false,
                pending_answer,
                cancel,
                permit,
            )
            .await
            {
                Some(_) => {}
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        }

        // The planner split the request: the task becomes a parent of one
        // child per part and never implements anything itself.
        if !draft.subtasks.is_empty() && task.parent.is_none() {
            let split = match check_subtasks(&draft.subtasks) {
                Ok(()) => split_into_subtasks(app, task_id, &draft.subtasks, auto_start_after_plan)
                    .await
                    .map_err(|e| format!("could not create the subtasks: {e}")),
                Err(reason) => Err(format!("subtasks ignored: {reason}")),
            };
            match split {
                Ok(()) => {
                    return if auto_start_after_plan {
                        PlanOutcome::Proceed
                    } else {
                        PlanOutcome::StoppedForReview
                    };
                }
                Err(why) => {
                    app.append_jev_decision(
                        task_id,
                        format!("Planner: {why}; running as one task"),
                    );
                    if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                        task = reloaded;
                    }
                }
            }
        }

        // Unconditional on an empty `verify` now (review item P2e): a
        // classifier that's off, missing a key, or simply unsure is no
        // reason to skip asking outright.
        if task.verify.is_empty() {
            let options = verify_options_from_package_json(&worktree);
            match ask_plan_question(
                app,
                task_id,
                "No verification command was found. Which command proves this task?",
                options,
                pending_answer,
                cancel,
                permit,
            )
            .await
            {
                Some(answer) => {
                    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                        t.verify = vec![answer];
                        t.updated_at = now_ms();
                        let _ = app.store.save_task(&t);
                        app.broadcast_task(&t);
                    }
                }
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        }

        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            t.status = if auto_start_after_plan {
                TaskStatus::Queued
            } else {
                TaskStatus::Stopped
            };
            t.updated_at = now_ms();
            let _ = app.store.save_task(&t);
            app.broadcast_task(&t);
        }
        return if auto_start_after_plan {
            PlanOutcome::Proceed
        } else {
            PlanOutcome::StoppedForReview
        };
    }
}

/// Turns a drafted task into a parent: one child per part, each drafted by
/// its own planner, branching from the parent's branch, with `dependsOn`
/// mapped from the parts' keys. The parent is left `running` (its loop then
/// starts the children) or `stopped` for the owner to review first.
async fn split_into_subtasks(
    app: &Arc<App>,
    parent_id: &str,
    parts: &[brief::PlanSubtask],
    start: bool,
) -> Result<(), String> {
    let (mut parts, mut serialised) = serialise_overlapping(parts);
    let (links, relayed) = relay_links(&mut parts);
    serialised.extend(relayed);
    let parts = parts.as_slice();
    let mut parent = app
        .store
        .load_task(parent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    let ids: HashMap<&str, String> = parts
        .iter()
        .map(|p| (p.key.trim(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let parent_mcp = std::fs::read_to_string(app.store.task_dir(parent_id).join("mcp.json")).ok();
    let titles_by_key: HashMap<&str, String> = parts
        .iter()
        .map(|p| {
            let title = if p.title.trim().is_empty() {
                truncate_chars(p.request.trim(), 60)
            } else {
                p.title.trim().to_string()
            };
            (p.key.trim(), title)
        })
        .collect();
    let now = now_ms();
    let mut titles = Vec::new();
    let mut children: Vec<Task> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let title = titles_by_key[part.key.trim()].clone();
        let created = app
            .create_task_record(NewTask {
                id: ids[part.key.trim()].clone(),
                repo_root: PathBuf::from(&parent.repo),
                title: title.clone(),
                goal: String::new(),
                criteria: vec![],
                verify: vec![],
                final_verify: vec![],
                checks: vec![],
                held_out: None,
                request: Some(subtask_request(&parent, &part.request)),
                branch: None,
                base: parent.branch.clone(),
                variant: parent.variant(),
                depends_on: part
                    .depends_on
                    .iter()
                    .map(|k| ids[k.trim()].clone())
                    .collect(),
                parent: Some(parent.id.clone()),
                paths: part.paths.clone(),
                relay_of: links[i].map(|p| ids[parts[p].key.trim()].clone()),
                // Eval reports count the parent, which carries the children's
                // cost.
                eval_set: None,
                eval_name: None,
                eval_check_cmd: None,
                // Keeps the planner's order when children are listed by age.
                created_at: now + i as i64,
            })
            .await;
        let child = match created {
            Ok(child) => child,
            Err(e) => {
                // All parts or none: a partial split would leave the parent
                // waiting on children that cover only part of the plan.
                discard_children(app, children).await;
                return Err(e);
            }
        };
        if let Some(mcp) = &parent_mcp {
            let _ = std::fs::write(app.store.task_dir(&child.id).join("mcp.json"), mcp);
        }
        titles.push(if part.depends_on.is_empty() {
            title
        } else {
            let after: Vec<&str> = part
                .depends_on
                .iter()
                .map(|k| titles_by_key[k.trim()].as_str())
                .collect();
            format!("{title} (after {})", after.join(", "))
        });
        children.push(child);
    }
    parent.decisions.extend(serialised);
    parent.decisions.push(format!(
        "Planner: split into {} subtasks: {}",
        parts.len(),
        titles.join("; ")
    ));
    parent.status = if start {
        TaskStatus::Running
    } else {
        TaskStatus::Stopped
    };
    parent.updated_at = now_ms();
    if let Err(e) = app.store.save_task(&parent) {
        discard_children(app, children).await;
        return Err(e.to_string());
    }
    for child in &children {
        app.broadcast_task(child);
    }
    app.broadcast_task(&parent);
    Ok(())
}
