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
        .filter(|d| {
            d.starts_with("Owner:") || d.starts_with("Lead:") || d.starts_with("Orchestrator:")
        })
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
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
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
        task.question = Some(Question::new(
            question_text,
            options,
            QuestionKind::PlanQuestion,
            AskedBy::Plan,
        ));
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        app.save_task_logged(&task);
        app.broadcast_task(&task);
        wake_orchestrator_for_waiting(app, &task);
    }

    let reply = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;
    let evidence = reply.as_ref().and_then(|r| r.default_evidence.clone());
    let like = reply.as_ref().and_then(|r| r.like.clone());
    let result = reply.map(|r| r.text);

    match &result {
        Some(answer) => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                // Peeked: the caller reads the mark again for its own wording.
                let caller = app
                    .agent_answers
                    .lock()
                    .unwrap()
                    .get(task_id)
                    .copied()
                    .unwrap_or(AnsweredBy::Owner);
                if caller != AnsweredBy::Owner || evidence.is_some() {
                    let question = task.question.take();
                    record_answer_for(
                        &mut task,
                        question.as_ref(),
                        answer,
                        caller,
                        evidence.as_deref(),
                        like.as_deref(),
                    );
                } else {
                    task.decisions
                        .push(format!("Owner: {question_text} -> {answer}"));
                    if let Some(question) = task.question.take() {
                        record_answered_question(&mut task, &question, answer, AnsweredBy::Owner);
                    }
                }
                task.status = TaskStatus::Drafting;
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            }
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                app.apply_cancellation(&mut task);
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(&mut task);
                }
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                app.advance_queue(&task.repo);
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
/// of the task's life -- planning runs at most once, ever. A task its parent
/// planner drafted completely (`planned_by_parent`) never runs it: the
/// parent's draft is its plan.
pub(super) fn needs_planning(task: &Task) -> bool {
    task.request.is_some()
        && !task.planned_by_parent
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
                    let prices = app.settings.read().unwrap().prices.clone();
                    settle_unfinished_cost(&mut t, idx, &app.store, &prices);
                    t.updated_at = now_ms();
                    app.save_task_logged(&t);
                    app.broadcast_task(&t);
                }
            }
        }
    }
    mark_stopped_if_not_already(app, task_id).await;
    PlanOutcome::Ended
}

/// A running plan attempt on `route`.
fn new_plan_attempt(n: u32, route: &Route, reason: &str) -> Attempt {
    Attempt {
        n,
        stage: Stage::Plan,
        route_id: route.id.clone(),
        harness: route.harness,
        model: route.model.clone().unwrap_or_default(),
        reason: reason.to_string(),
        session_id: None,
        pgid: None,
        started_at: now_ms(),
        ended_at: None,
        status: AttemptStatus::Running,
        summary: None,
        handoff: None,
        disputes: vec![],
        changed_files: vec![],
        verify: vec![],
        gate_blocks: 0,
        prefix_tokens: None,
        first_edit_at: None,
        review: None,
        failure: None,
        usage: None,
        cost_usd: None,
        cost_estimated: false,
        review_cost_usd: None,
        evidence: vec![],
        evidence_tree: None,
        review_tree: None,
        review_base: None,
        review_scope: None,
        evidence_base: None,
        evidence_from: None,
        advice: None,
        advisor_cost_usd: None,
        fingerprint: None,
        review_fingerprint: None,
        advisor_fingerprint: None,
        candidates: vec![],
        criteria_results: vec![],
        diff_stat: None,
    }
}

/// The run files of a read-only planner session: an empty MCP config and,
/// for Claude, read-only settings with the route's profile. The caller
/// deletes the key file when the run ends.
fn write_planner_run_files(
    app: &Arc<App>,
    route: &Route,
    settings: &Settings,
    run_dir: &Path,
) -> (PathBuf, PathBuf, PathBuf) {
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
            route,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    (mcp_path, settings_path, key_path)
}

/// Writes a parsed draft into the task and ends the plan attempt `idx` as
/// passed; the first draft and a corrected one go through the same code.
pub(super) fn store_draft(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    draft: &brief::PlanDraft,
    settings: &Settings,
    worktree: &Path,
) {
    task.title = draft.title.clone();
    task.goal = draft.goal.clone();
    task.planned_tier = draft.tier;
    // A whole-repo suite is no per-attempt check: it runs once, as a
    // finalVerify, on the final tree. Before the split, so it also applies
    // when review is off.
    let (suites, kept): (Vec<String>, Vec<String>) = draft
        .verify
        .iter()
        .cloned()
        .partition(|c| whole_repo_suite(worktree, c));
    // With review off nobody would check a moved entry: keep it as is.
    let (verify, judged) = if task.variant().review_route_id(settings).is_empty() {
        (kept, Vec::new())
    } else {
        split_verify_commands(worktree, &kept)
    };
    // What the planner says the task edits: a hint that orders sibling
    // subtasks. It never holds a lease; only files really changed do.
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
    // Stored before any split, so a splitting parent keeps them.
    task.checks = valid_checks(draft.checks.clone(), draft.criteria.len());
    task.held_out = valid_check(draft.held_out.clone(), draft.criteria.len());
    let (tests, dropped) = keep_planned_tests(&draft.tests);
    task.planner_tests.files = tests;
    for line in dropped {
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
    }
    task.final_verify = draft
        .final_verify
        .iter()
        .filter(|c| !c.trim().is_empty())
        .cloned()
        .collect();
    for suite in suites {
        if !task.final_verify.contains(&suite) {
            task.final_verify.push(suite.clone());
        }
        task.decisions.push(format!(
            "Orchestrator: verify entry {suite} is a whole-repo suite; moved to finalVerify"
        ));
    }
    task.record_planner_commands();
    task.brief_check.screenshot = draft
        .screenshot
        .clone()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    task.attempts[idx].status = AttemptStatus::Passed;
    task.attempts[idx].ended_at = Some(now_ms());
    task.attempts[idx].summary = Some(format!("Drafted: {}", draft.title));
    task.updated_at = now_ms();
    app.save_task_logged(task);
    app.broadcast_task(task);
}

/// Why a brief-check correction produced no draft.
enum Uncorrected {
    /// Draft from scratch instead; the reason is for the decision line.
    Fresh(String),
    /// The task was cancelled; the plan attempt at this index is open.
    Cancelled(usize),
    Failed,
}

/// A brief-check contradiction in plan attempt `idx`'s draft: resumes that
/// attempt's planner session with a short correction turn and stores the
/// corrected draft. No session, a failed run or an unparseable reply is
/// `Fresh`; a timed-out planner fails the task.
async fn correct_draft(
    app: &Arc<App>,
    task_id: &str,
    idx: usize,
    route: &Route,
    conflict: &str,
    cancel: &CancelToken,
) -> Result<brief::PlanDraft, Uncorrected> {
    let fresh = |why: &str| Uncorrected::Fresh(why.to_string());
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return Err(fresh("the task could not be read"));
    };
    let Some(session) = task.attempts.get(idx).and_then(|a| a.session_id.clone()) else {
        return Err(fresh("the planner attempt has no session id"));
    };
    if task.cost_budget().is_some_and(|b| task.cost_usd >= b) {
        return Err(fresh("the cost budget is reached"));
    }
    let settings = app.settings.read().unwrap().clone();
    let attempt_n = task.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
    task.attempts
        .push(new_plan_attempt(attempt_n, route, "brief correction"));
    let at = task.attempts.len() - 1;
    task.updated_at = now_ms();
    app.save_task_logged(&task);
    app.broadcast_task(&task);

    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(task_id, attempt_n).join("plan");
    let _ = std::fs::create_dir_all(&run_dir);
    let (mcp_path, settings_path, key_path) =
        write_planner_run_files(app, route, &settings, &run_dir);
    let req = harness::RunRequest {
        resume: Some(&session),
        ..review_request(route, &worktree, &mcp_path, &settings_path, &[])
    };
    let brief_text =
        brief::build_plan_correction_brief(conflict, &task.variant(), task.parent.is_none());
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        task_id,
        attempt_n,
        RunTrack::Attempt,
        &worktree,
        &req,
        CostTag::task("plan", &route.id),
        &brief_text,
        &run_dir.join("events.jsonl"),
        cancel,
        Guard::helper("planner", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
        task = reloaded;
    }
    let fail = |task: &mut Task, kind: FailureKind, why: String| {
        record_failure(task, at, kind, why.clone());
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
        Err(Uncorrected::Fresh(why))
    };
    let o = match result {
        Ok(o) => o,
        Err(RunError::Cancelled) => return Err(Uncorrected::Cancelled(at)),
        Err(RunError::Io(msg) | RunError::NotFound(msg)) => {
            return fail(
                &mut task,
                FailureKind::Error,
                format!("the resumed run could not start: {msg}"),
            );
        }
    };
    // A resumed Claude session's own total also covers the run before it.
    let own = match route.harness {
        Harness::Claude => o.streamed_cost(&settings.prices),
        Harness::Codex => None,
    };
    if let Some(cost) = own.or(o.cost_usd) {
        let a = &mut task.attempts[at];
        a.cost_usd = Some(a.cost_usd.unwrap_or(0.0) + cost);
        a.cost_estimated |= o.cost_estimated;
        task.cost_usd += cost;
    }
    task.attempts[at].fingerprint = o.fingerprint.clone();
    if o.timed_out {
        task.decisions
            .push(helper_timeout_message("planner", limit));
        record_failure(
            &mut task,
            at,
            FailureKind::Error,
            o.error.clone().unwrap_or_default(),
        );
        task.mark_failed();
        task.updated_at = now_ms();
        app.save_task_logged(&task);
        app.broadcast_task(&task);
        return Err(Uncorrected::Failed);
    }
    if let Some(err) = o.error {
        return fail(
            &mut task,
            FailureKind::Error,
            format!("the resumed run failed ({err})"),
        );
    }
    let Some(draft) = brief::parse_plan(&o.final_text.unwrap_or_default()) else {
        return fail(
            &mut task,
            FailureKind::NoDeliverable,
            "the resumed reply had no valid sushi-plan block".to_string(),
        );
    };
    store_draft(app, &mut task, at, &draft, &settings, &worktree);
    Ok(draft)
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
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
    auto_start_after_plan: bool,
) -> PlanOutcome {
    let entry_task = match app.store.load_task(task_id) {
        Ok(Some(task)) => task,
        _ => return end_plan_stage(app, task_id, None).await,
    };
    let (planner, decision) = {
        let settings = app.settings.read().unwrap();
        let variant = entry_task.variant();
        let (id, line) = variant.plan_route_for(&settings, &entry_task);
        (id.to_string(), line)
    };
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

        if !wait_for_harness(
            app,
            task_id,
            &mut task,
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
        if !task.decisions.contains(&decision) {
            task.decisions.push(decision.clone());
        }
        let Some(route) = settings.routes.iter().find(|r| r.id == planner).cloned() else {
            // An unconfigured planner id is treated exactly like planner ==
            // "" -- never a silent fallback to some route the owner never
            // chose for this.
            if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                t.mark_failed();
                t.updated_at = now_ms();
                app.save_task_logged(&t);
                app.broadcast_task(&t);
            }
            return end_plan_stage(app, task_id, None).await;
        };

        let Some((route, swap)) = app.usable_route(&settings, route) else {
            return end_plan_stage(app, task_id, None).await;
        };
        if let Some(line) = swap {
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }
        if variant.planner_route.is_some() {
            let line = variant_route_line("planner", &route.id);
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }

        let request_text = task.request.clone().unwrap_or_default();
        let attempt_n = task.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
        let worktree = PathBuf::from(&task.worktree);
        let run_dir = app.store.run_dir(task_id, attempt_n).join("plan");
        let _ = std::fs::create_dir_all(&run_dir);

        let attempt = new_plan_attempt(attempt_n, &route, "drafting");
        task.attempts.push(attempt);
        let idx = task.attempts.len() - 1;
        task.updated_at = now_ms();
        app.save_task_logged(&task);
        app.broadcast_task(&task);

        let (mcp_path, settings_path, key_path) =
            write_planner_run_files(app, &route, &settings, &run_dir);

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
        // A run that timed out or errored gets one resumed retry with twice
        // the time before the task fails.
        let mut resume_session: Option<String> = None;
        let mut doubled = false;
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
                app.save_task_logged(&task);
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
            // The single replan carries the gate's findings in its brief; the
            // request itself is never edited.
            let brief_text = if task.plan_gate.replanned {
                brief::with_block_before_report(
                    &brief_text,
                    &plan_gate::findings_block(&task.plan_gate.findings),
                )
            } else {
                brief_text
            };
            let tools = permissions::task_tools(
                app,
                &app.settings.read().unwrap().clone(),
                &task,
                Path::new(&task.worktree),
            );
            let brief_text = brief::with_block_before_report(
                &brief_text,
                &permissions::tools_block(&tools, true),
            );
            let brief_text = brief::with_block_before_report(
                &brief_text,
                &brief::scoped_checks_block(
                    &scoped_checks_for(&app.settings.read().unwrap(), &task),
                    &task.repo,
                ),
            );
            // A resumed session holds the request already: it continues.
            let brief_text = if resume_session.is_some() {
                brief::build_plan_continue_brief(&task.variant(), task.parent.is_none())
            } else {
                brief_text
            };
            let file_stem = if retry == 0 { "" } else { "-retry" };
            let _ = std::fs::write(run_dir.join(format!("brief{file_stem}.md")), &brief_text);
            let events_path = run_dir.join(format!("events{file_stem}.jsonl"));
            let limit = app.helper_timeout() * if doubled { 2 } else { 1 };
            let req = harness::RunRequest {
                resume: resume_session.as_deref(),
                ..review_request(&route, &worktree, &mcp_path, &settings_path, &[])
            };
            let run_result = run_harness(
                app,
                task_id,
                attempt_n,
                RunTrack::Attempt,
                &worktree,
                &req,
                CostTag::task("plan", &route.id),
                &brief_text,
                &events_path,
                cancel,
                Guard::helper("planner", limit),
            )
            .await;
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            match run_result {
                Ok(o) => {
                    if o.timed_out {
                        task.decisions
                            .push(helper_timeout_message("planner", limit));
                    }
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
                        if retry == 0 && !cancel.is_cancelled() {
                            task.decisions.push(format!(
                                "Planner: {err}; trying once more, resumed, with twice the time"
                            ));
                            resume_session = o.session_id.clone();
                            doubled = true;
                            continue;
                        }
                        record_failure(&mut task, idx, FailureKind::Error, err);
                        task.mark_failed();
                        task.updated_at = now_ms();
                        app.save_task_logged(&task);
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
                Err(RunError::Io(msg) | RunError::NotFound(msg)) => {
                    record_failure(&mut task, idx, FailureKind::Error, msg);
                    task.mark_failed();
                    task.updated_at = now_ms();
                    app.save_task_logged(&task);
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
            app.save_task_logged(&task);
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
                task.mark_failed();
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                return end_plan_stage(app, task_id, Some(idx)).await;
            }
            clarify_rounds += 1;
            app.save_task_logged(&task);
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
                    let who = match app.take_agent_answer(task_id) {
                        Some(AnsweredBy::Lead) => "Lead",
                        Some(AnsweredBy::Policy | AnsweredBy::Judge) => "Policy",
                        Some(_) => "Orchestrator",
                        None => "Owner",
                    };
                    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                        let combined = format!(
                            "{}\n\n({who} clarification: {answer})",
                            t.request.clone().unwrap_or_default()
                        );
                        t.request = Some(combined);
                        t.status = TaskStatus::Drafting;
                        t.updated_at = now_ms();
                        app.save_task_logged(&t);
                        app.broadcast_task(&t);
                    }
                    continue;
                }
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        };

        store_draft(app, &mut task, idx, &draft, &settings, &worktree);
        // The base checks of the draft run beside the brief check. A split
        // parent never implements, so its parts check their own commands.
        if draft.subtasks.is_empty() {
            start_base_checks(app, &mut task, cancel);
        }

        // Do the draft's own requirements contradict each other? One
        // redraft, once per task; the check runs again on the new draft.
        let mut draft = draft;
        match check_brief(app, task_id, true, cancel).await {
            BriefAction::Redraft(conflict) => {
                // The planner's own session fixes the contradiction when it
                // can; otherwise a fresh draft reads the request again.
                match correct_draft(app, task_id, idx, &route, &conflict, cancel).await {
                    Ok(corrected) => {
                        draft = corrected;
                        // The task has had its one redraft: a second
                        // conflict is kept for the implement brief.
                        if let BriefAction::Cancelled =
                            check_brief(app, task_id, true, cancel).await
                        {
                            return end_plan_stage(app, task_id, Some(idx)).await;
                        }
                    }
                    Err(Uncorrected::Failed) => return end_plan_stage(app, task_id, None).await,
                    Err(Uncorrected::Cancelled(at)) => {
                        return end_plan_stage(app, task_id, Some(at)).await;
                    }
                    Err(Uncorrected::Fresh(why)) => {
                        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                            let request = t.request.clone().unwrap_or_default();
                            t.request =
                                Some(format!("{request}\n\n{}", redraft_request_note(&conflict)));
                            t.decisions.push(format!(
                                "Brief check: the planner session was not resumed ({why}); drafting from scratch"
                            ));
                            t.updated_at = now_ms();
                            app.save_task_logged(&t);
                            app.broadcast_task(&t);
                        }
                        continue;
                    }
                }
            }
            BriefAction::Cancelled => return end_plan_stage(app, task_id, Some(idx)).await,
            BriefAction::Proceed => {}
        }
        // A redraft or a rewrite of the brief check starts the jobs again for
        // the commands it changed.
        if draft.subtasks.is_empty() {
            if let Ok(Some(mut rewritten)) = app.store.load_task(task_id) {
                start_base_checks(app, &mut rewritten, cancel);
            }
        }

        // The plan gate: one typed plan review, then the code checks. A
        // critical finding replans once.
        match plan_gate::run(app, task_id, attempt_n, &draft, cancel).await {
            plan_gate::GateAction::Replan => continue,
            plan_gate::GateAction::Cancelled => {
                return end_plan_stage(app, task_id, Some(idx)).await;
            }
            plan_gate::GateAction::Proceed => {}
        }
        if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
            task = reloaded;
        }

        let mut asked: Vec<&brief::PlanQuestion> = draft.questions.iter().take(3).collect();
        if task.variant().batch_questions {
            // A non-blocking question with a usable recommendation goes to
            // triage first; only one triage settles becomes an assumption,
            // the rest are asked with the blocking ones.
            let (routine, mut blocking): (Vec<_>, Vec<_>) = asked
                .into_iter()
                .partition(|q| !q.blocking && q.usable_recommendation().is_some());
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            for q in routine {
                // Planning reruns until a plan attempt passes, so the same
                // question can come back after an interruption: record it once.
                if task.assumptions.iter().any(|a| a.question == q.text) {
                    continue;
                }
                let asked_q = Question::new(
                    q.text.clone(),
                    q.options.clone(),
                    QuestionKind::PlanQuestion,
                    AskedBy::Plan,
                );
                let Some((answer, reason)) = triage_settles(
                    app, task_id, attempt_n, &mut task, &worktree, &asked_q, cancel,
                )
                .await
                else {
                    blocking.push(q);
                    continue;
                };
                let evidence = format!("{} Triage: {reason}", q.evidence);
                record_policy_answer(&mut task, &asked_q, &answer, "policy", &evidence, attempt_n);
                app.broadcast_log(
                    task_id,
                    attempt_n,
                    task.decisions.last().cloned().unwrap_or_default(),
                );
            }
            task.updated_at = now_ms();
            app.save_task_logged(&task);
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
            let failed = match check_subtasks(&draft.subtasks) {
                Err(reason) => Some(format!("subtasks ignored: {reason}")),
                Ok(()) => {
                    let plan = prepare_subtasks(&draft.subtasks);
                    // One merged group and nothing else: the task itself
                    // keeps its draft and gains the ordered checklist.
                    if plan.merge.separate.is_empty() {
                        merge_into_checklist(app, task_id, &plan).err()
                    } else {
                        match split_into_subtasks(app, task_id, &plan, auto_start_after_plan).await
                        {
                            Ok(()) => {
                                return if auto_start_after_plan {
                                    PlanOutcome::Proceed
                                } else {
                                    PlanOutcome::StoppedForReview
                                };
                            }
                            Err(e) => Some(format!("could not create the subtasks: {e}")),
                        }
                    }
                }
            };
            if let Some(why) = failed {
                app.append_decision(task_id, format!("Planner: {why}; running as one task"));
            }
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
        }

        // Unconditional on an empty `verify` (review item P2e): the first
        // command the repo offers becomes an assumption; the question is
        // asked only when the repo offers none.
        if task.verify.is_empty() {
            let mut options = verify_options(&worktree);
            let known = options.iter().position(|o| answer_policy::runs_ci_entry(o));
            if known.is_none() {
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
                            app.save_task_logged(&t);
                            app.broadcast_task(&t);
                        }
                    }
                    None => return end_plan_stage(app, task_id, Some(idx)).await,
                }
            } else if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                let command = options.swap_remove(known.unwrap_or_default());
                let question = "Which command proves this task?".to_string();
                app.broadcast_log(
                    task_id,
                    attempt_n,
                    format!("Planner assumed: {question} -> {command}"),
                );
                t.assumptions.push(Assumption {
                    question,
                    answer: command.clone(),
                    evidence:
                        "the first verification command found in the repo that runs the whole suite"
                            .into(),
                    by: "planner".to_string(),
                    kind: None,
                    attempt: None,
                    commands: Vec::new(),
                    overturned: false,
                    owner_answer: None,
                });
                t.verify = vec![command];
                t.updated_at = now_ms();
                app.save_task_logged(&t);
                app.broadcast_task(&t);
            }
        }

        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            t.status = if auto_start_after_plan {
                TaskStatus::Queued
            } else {
                TaskStatus::Stopped
            };
            if t.status == TaskStatus::Stopped {
                app.close_stopped_task(&mut t);
            }
            t.updated_at = now_ms();
            app.save_task_logged(&t);
            app.broadcast_task(&t);
        }
        return if auto_start_after_plan {
            PlanOutcome::Proceed
        } else {
            PlanOutcome::StoppedForReview
        };
    }
}

pub(super) struct SubtaskPlan {
    pub(super) parts: Vec<brief::PlanSubtask>,
    pub(super) links: Vec<Option<usize>>,
    decisions: Vec<String>,
    pub(super) merge: plan_gate::MergePlan,
}

/// All dependency edges are present before grouping can contract the graph.
pub(super) fn prepare_subtasks(parts: &[brief::PlanSubtask]) -> SubtaskPlan {
    let (mut parts, mut decisions) = serialise_overlapping(parts);
    let (links, relayed) = relay_links(&mut parts);
    decisions.extend(relayed);
    let merge = plan_gate::merge_parts(&parts);
    SubtaskPlan {
        parts,
        links,
        decisions,
        merge,
    }
}

/// The whole plan is one merged group: the task keeps its draft and gains
/// the ordered checklist. `Err("")` tells the caller to carry on as one task.
fn merge_into_checklist(app: &Arc<App>, task_id: &str, plan: &SubtaskPlan) -> Result<(), String> {
    let mut task = app
        .store
        .load_task(task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    task.checklist = plan_gate::checklist_of(&plan.parts, &plan.merge.merged);
    task.decisions.extend(plan.decisions.iter().cloned());
    task.decisions.push(format!(
        "Plan gate: merged {} subtasks into one task with a checklist",
        plan.merge.merged.len()
    ));
    task.updated_at = now_ms();
    app.store.save_task(&task).map_err(|e| e.to_string())?;
    app.broadcast_task(&task);
    Ok(())
}

/// What a standalone part brings to its child: its own draft.
#[derive(Default)]
struct Draft<'a> {
    goal: String,
    criteria: Vec<String>,
    verify: Vec<String>,
    final_verify: Vec<String>,
    checks: Vec<Check>,
    held_out: Option<Check>,
    complete: bool,
    part: Option<&'a brief::PlanSubtask>,
}

/// One child of a split: the planner's parts that stand alone, and the one
/// merged group (if any) with its ordered checklist.
struct Unit {
    /// Indexes into the parts, in group order.
    members: Vec<usize>,
    merged: bool,
}

/// Turns a drafted task into a parent: one child per unit, branching from
/// the parent's branch, with `dependsOn` mapped from the parts' keys. A
/// standalone part the planner drafted completely (title, goal, criteria and
/// verify) runs as drafted; an incomplete one, and the merged group, is
/// drafted by its own planner from its request. The parent is left `running` (its loop then
/// starts the children) or `stopped` for the owner to review first.
async fn split_into_subtasks(
    app: &Arc<App>,
    parent_id: &str,
    plan: &SubtaskPlan,
    start: bool,
) -> Result<(), String> {
    let parts = plan.parts.as_slice();
    let merge = &plan.merge;
    let links = &plan.links;
    let mut parent = app
        .store
        .load_task(parent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    // Planner order, the merged group at its first member's place.
    let mut units: Vec<Unit> = Vec::new();
    for i in 0..parts.len() {
        if merge.merged.contains(&i) {
            if merge.merged.iter().min() == Some(&i) {
                units.push(Unit {
                    members: merge.merged.clone(),
                    merged: true,
                });
            }
        } else {
            units.push(Unit {
                members: vec![i],
                merged: false,
            });
        }
    }
    let unit_ids: Vec<String> = units
        .iter()
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect();
    let id_of_key: HashMap<&str, &String> = units
        .iter()
        .zip(&unit_ids)
        .flat_map(|(u, id)| u.members.iter().map(move |&m| (parts[m].key.trim(), id)))
        .collect();
    let part_title = |p: &brief::PlanSubtask| {
        if p.title.trim().is_empty() {
            truncate_chars(p.request.trim(), 60)
        } else {
            p.title.trim().to_string()
        }
    };
    let unit_title = |u: &Unit| {
        let first = part_title(&parts[u.members[0]]);
        if u.members.len() > 1 {
            format!("{first} (+{} more)", u.members.len() - 1)
        } else {
            first
        }
    };
    let parent_mcp = std::fs::read_to_string(app.store.task_dir(parent_id).join("mcp.json")).ok();
    let now = now_ms();
    let review_on = !app.settings.read().unwrap().review.is_empty();
    let parent_worktree = PathBuf::from(&parent.worktree);
    let mut titles = Vec::new();
    let mut children: Vec<Task> = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        let title = unit_title(unit);
        let mut depends_on: Vec<String> = Vec::new();
        for &m in &unit.members {
            for d in &parts[m].depends_on {
                let id = id_of_key[d.trim()].clone();
                if id != unit_ids[i] && !depends_on.contains(&id) {
                    depends_on.push(id);
                }
            }
        }
        let non_empty = |list: &[String]| -> Vec<String> {
            list.iter()
                .filter(|c| !c.trim().is_empty())
                .cloned()
                .collect()
        };
        // A merged group is drafted by its own planner; a standalone part
        // carries the draft it came with.
        let mut drafted = Draft::default();
        let (request, checklist, paths) = if unit.merged {
            let text = unit
                .members
                .iter()
                .map(|&m| parts[m].request.trim())
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut paths: Vec<String> = Vec::new();
            for &m in &unit.members {
                for p in &parts[m].paths {
                    if !paths.contains(p) {
                        paths.push(p.clone());
                    }
                }
            }
            (text, plan_gate::checklist_of(parts, &unit.members), paths)
        } else {
            let part = &parts[unit.members[0]];
            let mut drafted_verify = non_empty(&part.verify);
            // A complete draft leads with the part's targeted check, so the
            // subtask runs that check (the full CI entry is the parent's).
            let check = part.check.trim();
            if part.is_complete()
                && !check.is_empty()
                && !drafted_verify.iter().any(|c| c.trim() == check)
            {
                drafted_verify.insert(0, check.to_string());
            }
            let (verify, judged) = if review_on {
                split_verify_commands(&parent_worktree, &drafted_verify)
            } else {
                (drafted_verify, Vec::new())
            };
            let mut criteria = non_empty(&part.criteria);
            // Indices count the part's own criteria, before the review entries.
            let own_criteria = criteria.len();
            drafted.checks = valid_checks(part.checks.clone(), own_criteria);
            drafted.held_out = valid_check(part.held_out.clone(), own_criteria);
            criteria.extend(
                judged
                    .into_iter()
                    .map(|v| format!("Checked by review (not a shell command): {v}")),
            );
            drafted.goal = part.goal.trim().to_string();
            drafted.complete = part.is_complete() && !verify.is_empty();
            drafted.criteria = criteria;
            drafted.verify = verify;
            drafted.final_verify = non_empty(&part.final_verify);
            drafted.part = Some(part);
            (part.request.trim().to_string(), vec![], part.paths.clone())
        };
        let relay_of = links[unit.members[0]].and_then(|at| {
            let id = id_of_key[parts[at].key.trim()];
            (id != &unit_ids[i]).then(|| id.clone())
        });
        let created = app
            .create_task_record(NewTask {
                backlog: None,
                id: unit_ids[i].clone(),
                repo_root: PathBuf::from(&parent.repo),
                project_id: parent.project_id.clone(),
                title: title.clone(),
                goal: drafted.goal,
                criteria: drafted.criteria,
                verify: drafted.verify,
                final_verify: drafted.final_verify,
                checks: drafted.checks,
                held_out: drafted.held_out,
                request: Some(subtask_request(&parent, &request)),
                branch: None,
                base: parent.branch.clone(),
                base_from: fresh_base::BaseFrom::Plain,
                variant: parent.variant(),
                depends_on: depends_on.clone(),
                parent: Some(parent.id.clone()),
                paths,
                checklist,
                relay_of,
                // Eval reports count the parent, which carries the children's
                // cost.
                eval_set: None,
                eval_name: None,
                eval_check_cmd: None,
                source: Some("planner".to_string()),
                // Keeps the planner's order when children are listed by age.
                created_at: now + i as i64,
            })
            .await;
        let mut child = match created {
            Ok(child) => child,
            Err(e) => {
                // All parts or none: a partial split would leave the parent
                // waiting on children that cover only part of the plan.
                discard_children(app, children).await;
                return Err(e);
            }
        };
        if let Some(part) = drafted.part {
            if !unit.merged {
                let (tests, dropped) = keep_planned_tests(&part.tests);
                child.planner_tests.files = tests;
                child.decisions.extend(dropped);
            }
            child.planned_tier = part.tier;
            child.visual_criteria = part.visual_criteria.clone();
            child.brief_check.screenshot = part
                .screenshot
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            let check = part.check.trim();
            child.brief_check.check = (!check.is_empty()).then(|| check.to_string());
            child.planned_by_parent = drafted.complete;
            if drafted.complete {
                child.record_planner_commands();
            }
        }
        if let Err(e) = app.store.save_task(&child) {
            children.push(child);
            discard_children(app, children).await;
            return Err(e.to_string());
        }
        if let Some(mcp) = &parent_mcp {
            let _ = std::fs::write(app.store.task_dir(&child.id).join("mcp.json"), mcp);
        }
        titles.push(if depends_on.is_empty() {
            title
        } else {
            let after: Vec<String> = depends_on
                .iter()
                .filter_map(|id| unit_ids.iter().position(|u| u == id))
                .map(|at| unit_title(&units[at]))
                .collect();
            format!("{title} (after {})", after.join(", "))
        });
        children.push(child);
    }
    parent.decisions.extend(plan.decisions.iter().cloned());
    if merge.merged.len() > 1 {
        parent.decisions.push(format!(
            "Plan gate: merged {} subtasks into one task with a checklist",
            merge.merged.len()
        ));
    }
    parent.decisions.push(format!(
        "Planner: split into {} subtasks: {}",
        units.len(),
        titles.join("; ")
    ));
    parent.status = if start {
        TaskStatus::Running
    } else {
        TaskStatus::Stopped
    };
    if parent.status == TaskStatus::Stopped {
        app.close_stopped_task(&mut parent);
    }
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

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn planner_timed_out_is_tried_once_more_with_twice_the_time_then_fails() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.status = TaskStatus::Drafting;
        task.attempts.clear();
        task.request = Some("Ship the change".into());
        app.store.save_task(&task).unwrap();
        let pending = Arc::new(StdMutex::new(None));
        let mut permit = None;
        let result = run_plan_stage(
            &app,
            &task.id,
            &pending,
            &CancelToken::new(),
            &mut permit,
            true,
        )
        .await;
        assert!(matches!(result, PlanOutcome::Ended));
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(
            task.attempts[0].failure.as_ref().unwrap().kind,
            FailureKind::Error
        );
        // The retry ran with twice the limit and timed out too.
        assert_eq!(
            task.attempts[0].failure.as_ref().unwrap().detail,
            "planner timed out after 0.5s"
        );
        let first = helper_timeout_message("planner", Duration::from_millis(250));
        let second = helper_timeout_message("planner", Duration::from_millis(500));
        for line in [first, second] {
            assert_eq!(
                task.decisions.iter().filter(|d| **d == line).count(),
                1,
                "{:?}",
                task.decisions
            );
        }
        assert!(
            task.decisions
                .iter()
                .any(|d| d.contains("trying once more, resumed, with twice the time")),
            "{:?}",
            task.decisions
        );
        // Never a third run.
        assert!(chat::fake_overseer_calls(Path::new(&task.worktree)) <= 2);
    }

    #[tokio::test]
    async fn planner_correction_timed_out_fails_instead_of_redrafting() {
        let (app, _dir, mut task, route) = slow_helper_task();
        task.attempts[0].stage = Stage::Plan;
        task.attempts[0].session_id = Some("s".into());
        app.store.save_task(&task).unwrap();
        let result =
            correct_draft(&app, &task.id, 0, &route, "conflict", &CancelToken::new()).await;
        assert!(matches!(result, Err(Uncorrected::Failed)));
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(
            task.attempts[1].failure.as_ref().unwrap().kind,
            FailureKind::Error
        );
        assert_ne!(task.attempts[0].n, task.attempts[1].n);
        assert_helper_timeout(&task, "planner");
    }
}
