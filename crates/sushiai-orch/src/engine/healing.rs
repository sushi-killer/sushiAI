//! Self-healing briefs: what orchd changes in a task's own contract, without
//! the owner, when the contract cannot be met as written. Every automatic
//! change is a decision line and an assumption (`by: "orchd"`) the owner can
//! overturn. The judge runs use `settings.briefCheckRoute`, like the brief
//! check; without that route nothing here rewrites anything.

use super::*;

/// The most follow-ups a task keeps for its report.
const MAX_FOLLOW_UPS: usize = 12;
const MAX_REASON_CHARS: usize = 300;

/// The route of the cheap judge runs; `None` when they are off.
pub(super) fn judge_route(app: &App, variant: &Variant) -> Option<Route> {
    check_route_for(app, variant)?.1
}

/// The outermost JSON object of a judge's reply, whatever surrounds it.
fn judge_json(text: &str) -> Option<serde_json::Value> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn reason_of(v: &serde_json::Value) -> String {
    let r = str_field(v, "reason");
    if r.is_empty() {
        "no reason given".to_string()
    } else {
        truncate_chars(&r, MAX_REASON_CHARS)
    }
}

/// Lowercase alphanumerics only: findings quote criteria loosely.
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn overturnable(task: &mut Task, question: String, answer: String, evidence: String) {
    task.assumptions.push(Assumption {
        question,
        answer,
        evidence,
        by: "orchd".to_string(),
        kind: None,
        attempt: None,
        commands: Vec::new(),
        overturned: false,
        owner_answer: None,
    });
}

/// Replaces criterion `i` with `text`, keeping its visual mark. A judge may
/// not take a shared check out of the criterion's `-- check:` part
/// (`removal_refusal`): then nothing changes and the reason comes back.
fn amend_criterion(
    settings: &Settings,
    task: &mut Task,
    i: usize,
    text: &str,
) -> Result<(), String> {
    if let Some(why) = criterion_text_refusal(settings, task, i, text) {
        return Err(why);
    }
    let old = std::mem::replace(&mut task.criteria[i], text.to_string());
    for v in task.visual_criteria.iter_mut().filter(|v| **v == old) {
        *v = text.to_string();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 1. Feasibility of a check on the base
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(super) fn amend_criterion_for_test(
    settings: &Settings,
    task: &mut Task,
    i: usize,
    text: &str,
) -> Result<(), String> {
    amend_criterion(settings, task, i, text)
}

const FEASIBILITY_PROMPT: &str = "You judge whether a check command of a coding task can work in this repository. The command was run on the repository as it is before any work and exited non-zero. You may read files in the working directory but change nothing.\n\nAnswer with one verdict:\n- `feasible`: it fails as it should, because the task is meant to make it pass (for example a test for the new behaviour).\n- `rewrite`: the repository cannot run this kind of check (the test runner, harness, script or tool it needs does not exist here). Give `command`, the nearest check this repository can really run and that exits 0 only when the criterion holds, and `criterionText` when the criterion's own `-- check:` wording must change with it.\n- `unrelated`: it fails for a reason that has nothing to do with the task (a broken environment, a flaky or already broken test, a missing credential).\n\nReply with only JSON: {\"verdict\": \"feasible|rewrite|unrelated\", \"command\": \"\", \"criterion\": <index or null>, \"criterionText\": \"\", \"reason\": \"<one sentence>\"}";

#[derive(Clone)]
pub(super) enum Feasibility {
    /// Fails as it should.
    AsWritten,
    Rewrite {
        command: String,
        criterion: Option<(usize, String)>,
        reason: String,
    },
    Unrelated {
        reason: String,
    },
}

pub(super) fn feasibility_prompt(task: &Task, failed: &VerifyOutcome) -> String {
    let criteria: Vec<String> = task
        .criteria
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{i}: {c}"))
        .collect();
    format!(
        "{FEASIBILITY_PROMPT}\n\n{}\n",
        json!({
            "goal": task.goal,
            "criteria": criteria,
            "command": failed.command,
            "exitCode": failed.code,
            "output": tail_chars(failed.tail.trim(), 1500),
        })
    )
}

pub(super) fn parse_feasibility(text: &str, criteria: usize) -> Option<Feasibility> {
    let v = judge_json(text)?;
    match str_field(&v, "verdict").to_ascii_lowercase().as_str() {
        "feasible" => Some(Feasibility::AsWritten),
        "unrelated" => Some(Feasibility::Unrelated {
            reason: reason_of(&v),
        }),
        "rewrite" => {
            let command = str_field(&v, "command");
            if command.is_empty() {
                return None;
            }
            let text = str_field(&v, "criterionText");
            let criterion = v
                .get("criterion")
                .and_then(|i| i.as_u64())
                .map(|i| i as usize)
                .filter(|i| *i < criteria && !text.is_empty())
                .map(|i| (i, text));
            Some(Feasibility::Rewrite {
                command,
                criterion,
                reason: reason_of(&v),
            })
        }
        _ => None,
    }
}

/// `grep` reads a pattern that starts with `-` as an option, so such a check
/// cannot run. Returns `command` with `--` inserted before each such
/// pattern, or `None` when nothing needs it. Quote-aware, and it leaves
/// `-e`/`-f` forms alone (their next word is already an argument).
pub(super) fn fix_grep_dashes(command: &str) -> Option<String> {
    // Words with their byte offsets; quotes keep a word whole.
    let mut words: Vec<(usize, String, bool)> = Vec::new();
    let mut cur: Option<(usize, String, bool)> = None;
    let mut quote: Option<char> = None;
    for (i, c) in command.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.get_or_insert((i, String::new(), false)).1.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.get_or_insert((i, String::new(), true)).2 = true;
            }
            (None, c) if c.is_whitespace() => words.extend(cur.take()),
            (None, c) => cur.get_or_insert((i, String::new(), false)).1.push(c),
        }
    }
    words.extend(cur);
    let mut inserts: Vec<usize> = Vec::new();
    let mut at_start = true;
    let mut i = 0;
    while i < words.len() {
        let w = words[i].1.as_str();
        if matches!(w, "&&" | "||" | ";" | "|" | "(") {
            at_start = true;
            i += 1;
            continue;
        }
        if !(at_start && matches!(w, "grep" | "egrep" | "fgrep")) {
            at_start = false;
            i += 1;
            continue;
        }
        at_start = false;
        i += 1;
        let mut takes_arg = false;
        while i < words.len() {
            let o = words[i].1.as_str();
            // `--long` is an option quoted or not; a quoted word with one
            // dash is the pattern.
            if words[i].2 && !o.starts_with("--") {
                break;
            }
            if o == "--" {
                i = words.len();
                takes_arg = true;
                break;
            }
            let short_cluster = o.starts_with('-') && !o.starts_with("--") && o.len() > 1;
            if short_cluster && o[1..].contains(['e', 'f']) {
                takes_arg = true;
                break;
            }
            if o.starts_with('-') && o.len() > 1 {
                i += 1;
                continue;
            }
            break;
        }
        if !takes_arg && i < words.len() && words[i].1.starts_with('-') && words[i].1.len() > 1 {
            inserts.push(words[i].0);
            i += 1;
        }
    }
    if inserts.is_empty() {
        return None;
    }
    let mut out = command.to_string();
    for at in inserts.into_iter().rev() {
        out.insert_str(at, "-- ");
    }
    Some(out)
}

/// Fixes, before any check runs, each grep check whose pattern starts with
/// `-`; one decision line per command. Returns whether anything changed.
pub(super) fn heal_grep_dashes(task: &mut Task) -> bool {
    let mut changed: Vec<(String, String)> = Vec::new();
    for c in task
        .verify
        .iter_mut()
        .chain(task.final_verify.iter_mut())
        .chain(task.checks.iter_mut().map(|c| &mut c.run))
    {
        if let Some(fixed) = fix_grep_dashes(c) {
            if !changed.iter().any(|(old, _)| old == c) {
                changed.push((c.clone(), fixed.clone()));
            }
            *c = fixed;
        }
    }
    for (old, new) in &changed {
        task.decisions.push(format!(
            "Orchestrator: check {old} could not run (grep reads a pattern starting with `-` as an option); rewritten to {new}"
        ));
    }
    !changed.is_empty()
}

/// Applies a ruling about `command`; `false` when it changed nothing.
pub(super) fn apply_feasibility(
    settings: &Settings,
    task: &mut Task,
    command: &str,
    ruling: Feasibility,
) -> bool {
    // A ruling is never the owner's: a shared check keeps gating as written
    // and reaches the twice-on-base question.
    if !matches!(ruling, Feasibility::AsWritten) {
        if let Some(why) = removal_refusal(settings, task, &[command.to_string()], Origin::Other) {
            task.decisions.push(format!(
                "Orchestrator: check {command} keeps gating as written: {why}"
            ));
            return true;
        }
    }
    match ruling {
        Feasibility::AsWritten => false,
        Feasibility::Rewrite {
            command: new,
            criterion,
            reason,
        } => {
            if new == command {
                return false;
            }
            if let Some((i, text)) = &criterion {
                if let Some(why) = criterion_text_refusal(settings, task, *i, text) {
                    task.decisions.push(format!(
                        "Orchestrator: check {command} keeps gating as written: {why}"
                    ));
                    return true;
                }
            }
            for c in task
                .verify
                .iter_mut()
                .chain(task.final_verify.iter_mut())
                .chain(task.checks.iter_mut().map(|c| &mut c.run))
                .filter(|c| c.as_str() == command)
            {
                *c = new.clone();
            }
            let mut line = format!(
                "Orchestrator: check {command} cannot run on this repository ({reason}); rewritten to {new}"
            );
            if let Some((i, text)) = criterion {
                line.push_str(&format!("; criterion amended to: {text}"));
                let _ = amend_criterion(settings, task, i, &text);
            }
            task.decisions.push(line);
            overturnable(
                task,
                format!("Can the check `{command}` run on this repository?"),
                format!("No: rewritten to `{new}`"),
                reason,
            );
            true
        }
        Feasibility::Unrelated { reason } => {
            task.verify.retain(|c| c != command);
            task.final_verify.retain(|c| c != command);
            task.checks.retain(|c| c.run != command);
            if !task.brief_check.non_gating.iter().any(|c| c == command) {
                task.brief_check.non_gating.push(command.to_string());
            }
            task.decisions.push(format!(
                "Orchestrator: check {command} fails on the base for a reason unrelated to the task ({reason}); non-gating"
            ));
            overturnable(
                task,
                format!("Should `{command}` gate this task?"),
                "No: it fails on the base for an unrelated reason".to_string(),
                reason,
            );
            true
        }
    }
}

/// What the background base preflight found; the loop applies it to its own
/// task (`apply_base_preflight`), the job itself writes no task state.
#[derive(Clone, Default)]
pub(super) struct BasePreflight {
    /// The base could be checked out and the judge ran over every failure:
    /// feasibility is settled for good.
    settled: bool,
    rulings: Vec<(VerifyOutcome, Feasibility)>,
    /// Decision lines the plan-kept rule wrote (`keep_planned_checks`).
    decisions: Vec<String>,
    cost: f64,
}

/// One task's base preflight: its result arrives on `result`; the channel
/// closes without a value when the job was cancelled.
pub(super) struct PreflightJob {
    result: tokio::sync::watch::Receiver<Option<Arc<BasePreflight>>>,
    applied: std::sync::atomic::AtomicBool,
    /// The commands the job runs on the base; a job for other commands is
    /// replaced, not reused.
    commands: Vec<String>,
    cancel: CancelToken,
}

impl PreflightJob {
    /// The result, waiting for the job when it still runs; `None` when it
    /// was cancelled.
    async fn wait(&self) -> Option<Arc<BasePreflight>> {
        let mut rx = self.result.clone();
        let seen = rx.wait_for(|v| v.is_some()).await.ok()?;
        seen.clone()
    }

    fn running(&self) -> bool {
        self.result.borrow().is_none() && self.result.has_changed().is_ok()
    }

    fn finished(&self) -> bool {
        self.result.borrow().is_some()
    }
}

/// The commands the base preflight runs for `task`: every `verify` and
/// `checks` command, once each (`finalVerify` has its own base jobs).
fn preflight_commands(task: &Task) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for c in task.verify.iter().chain(task.checks.iter().map(|c| &c.run)) {
        if !c.trim().is_empty() && !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    commands
}

/// Starts the base preflight of `task` in the background, holding no slot:
/// every `verify` and `checks` command runs on the base once (`run_on_base`)
/// and each failure goes to the feasibility judge. The result only goes into
/// the app's map; nothing is saved here. A no-op while a job of this task
/// for the same commands is running or finished; a job for other commands
/// (the draft or the brief check rewrote them) is cancelled and started
/// again. `loop_cancel` stops the job.
pub(super) fn spawn_base_preflight(app: &Arc<App>, task: &Task, loop_cancel: &CancelToken) {
    let commands = preflight_commands(task);
    let mut jobs = app.base_preflights.lock().unwrap();
    if let Some(job) = jobs.get(&task.id) {
        if job.commands == commands && (job.running() || job.finished()) {
            return;
        }
        job.cancel.cancel();
    }
    let (tx, rx) = tokio::sync::watch::channel(None);
    // The loop's cancel reaches the job through the job's own token, which
    // the base runs watch, so their scratch checkout is cleaned up.
    let cancel = CancelToken::new();
    jobs.insert(
        task.id.clone(),
        Arc::new(PreflightJob {
            result: rx,
            applied: std::sync::atomic::AtomicBool::new(false),
            commands,
            cancel: cancel.clone(),
        }),
    );
    drop(jobs);
    let (app, task, loop_cancel) = (app.clone(), task.clone(), loop_cancel.clone());
    tokio::spawn(async move {
        let watcher = {
            let job_cancel = cancel.clone();
            tokio::spawn(async move {
                loop_cancel.cancelled().await;
                job_cancel.cancel();
            })
        };
        let result = run_base_preflight(&app, &task, &cancel).await;
        watcher.abort();
        if let Some(result) = result {
            let _ = tx.send(Some(Arc::new(result)));
        }
    });
}

/// Starts the base jobs of `task` that its brief check does not need to
/// wait for: the preflight of its `verify` and `checks` and the base runs of
/// its `finalVerify`. Called as soon as the commands exist and again after a
/// rewrite; a job for commands that did not change is not started twice.
pub(super) fn start_base_checks(app: &Arc<App>, task: &mut Task, cancel: &CancelToken) {
    if task.brief_check.feasibility_done {
        return;
    }
    if heal_grep_dashes(task) {
        app.verify_cache.lock().unwrap().remove(&task.id);
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    spawn_base_preflight(app, task, cancel);
    start_final_base_jobs(app, task);
}

/// The job body. `None`: cancelled.
async fn run_base_preflight(
    app: &Arc<App>,
    task: &Task,
    cancel: &CancelToken,
) -> Option<BasePreflight> {
    let mut out = BasePreflight::default();
    // No judge: nothing is ruled and nothing is settled.
    let Some(route) = judge_route(app, &task.variant()) else {
        return Some(out);
    };
    // `finalVerify` is not judged here: its base runs are the background
    // jobs of `start_final_base_jobs` and the finals gate after review. A
    // command that is in `verify` as well is still judged.
    let commands = preflight_commands(task);
    let run_dir = match app
        .store
        .claim_helper_run_dir(&task.id, "feasibility-base", None)
    {
        Ok(dir) => dir,
        Err(_) => return Some(out),
    };
    let ran = run_on_base(
        app,
        Some(&task.id),
        &task.repo,
        &task.base_sha,
        &commands,
        &run_dir,
        cancel,
    )
    .await;
    if cancel.is_cancelled() {
        return None;
    }
    // A base that could not be checked out says nothing: not settled.
    let Ok(results) = ran else {
        return Some(out);
    };
    // A check that names files the plan creates is feasible by rule: no
    // judge run, only the kept decision.
    let failing: Vec<String> = results
        .iter()
        .filter(|r| r.code != Some(0) && !timed_out_under_load(r))
        .map(|r| r.command.clone())
        .collect();
    let mut scratch = task.clone();
    record_check_outcomes(app, &mut scratch, &results);
    let kept = keep_planned_checks(app, &mut scratch, &failing);
    out.decisions = scratch.decisions[task.decisions.len()..].to_vec();
    for failed in results
        .iter()
        .filter(|r| r.code != Some(0) && !timed_out_under_load(r) && !kept.contains(&r.command))
    {
        let prompt = feasibility_prompt(task, failed);
        let reply = run_judge(
            app,
            task,
            &route,
            None,
            "brief_check",
            "feasibility",
            &prompt,
            cancel,
        )
        .await;
        match reply {
            Err(RunError::Cancelled) => return None,
            Ok(JudgeRun {
                reply,
                cost,
                timeout_note,
            }) => {
                out.decisions.extend(timeout_note);
                out.cost += cost;
                if let Some(r) = reply
                    .ok()
                    .and_then(|text| parse_feasibility(&text, task.criteria.len()))
                {
                    out.rulings.push((failed.clone(), r));
                }
            }
            Err(RunError::Io(_) | RunError::NotFound(_)) => {}
        }
    }
    out.settled = true;
    Some(out)
}

/// Waits for the task's base preflight (a running one is awaited and the wait
/// is a decision line) and applies it to the loop's own `task`: the judge's
/// cost, the rulings (`apply_feasibility`'s exceptions hold: exit 127 keeps
/// gating, a shared check is never switched off) and, when the judge settled,
/// `feasibility_done`. Saves and broadcasts. A no-op without a job or after
/// it was applied. `false`: cancelled while waiting.
pub(super) async fn settle_base_preflight(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> bool {
    let Some(job) = app.base_preflights.lock().unwrap().get(&task.id).cloned() else {
        return true;
    };
    if job.applied.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let waited = job.running();
    let result = tokio::select! {
        r = job.wait() => r,
        _ = cancel.cancelled() => return false,
    };
    if job.applied.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let Some(result) = result else {
        // A cancelled job: forget it, a later start may spawn a new one.
        app.base_preflights.lock().unwrap().remove(&task.id);
        return true;
    };
    if waited {
        task.decisions.push(
            "Orchestrator: waited for the base preflight before the final checks".to_string(),
        );
    }
    apply_base_preflight(app, task, &result);
    app.save_task_logged(task);
    app.broadcast_task(task);
    true
}

fn apply_base_preflight(app: &Arc<App>, task: &mut Task, result: &BasePreflight) {
    task.cost_usd += result.cost;
    for line in &result.decisions {
        if !task.decisions.contains(line) {
            task.decisions.push(line.clone());
        }
    }
    let mut changed = false;
    for (failed, ruling) in &result.rulings {
        // 127: the shell could not find a program. A rewrite to a check the
        // repo can run is fine, but "unrelated" would be this host's PATH or
        // toolchain quietly switching the check off: it keeps gating.
        if failed.code == Some(127) && matches!(ruling, Feasibility::Unrelated { .. }) {
            task.decisions.push(format!(
                "Orchestrator: check {} cannot find a program on this host (exit 127); it keeps gating - fix the host's PATH or toolchain",
                failed.command
            ));
            changed = true;
            continue;
        }
        let settings = app.settings.read().unwrap().clone();
        changed |= apply_feasibility(&settings, task, &failed.command, ruling.clone());
    }
    if changed {
        app.verify_cache.lock().unwrap().remove(&task.id);
    }
    if result.settled {
        task.brief_check.feasibility_done = true;
    }
    task.updated_at = now_ms();
}

// ---------------------------------------------------------------------------
// 2. The same criterion unmet twice
// ---------------------------------------------------------------------------

const RULING_PROMPT: &str = "You judge why a coding task keeps failing review on the same acceptance criterion: was the criterion not implemented (`code gap`), or can it not be met or proven as written in this repository (`infeasible as written`, for example it asks for a kind of test the repository has no harness for, or for something the reviewer cannot observe)? You may read files in the working directory but change nothing. Two review rejections of the criterion are given.\n\nWhen it is infeasible, give `criterion`: the same requirement in the nearest form this repository can really check, keeping its `-- check:` part feasible. Reply with only JSON: {\"verdict\": \"code gap|infeasible as written\", \"criterion\": \"\", \"reason\": \"<one sentence>\"}";

pub(super) enum Ruling {
    CodeGap,
    Infeasible { criterion: String, reason: String },
}

fn strip_evidence(finding: &str) -> &str {
    finding
        .strip_prefix("Unmet criterion:")
        .unwrap_or(finding)
        .trim()
}

/// The criterion an "Unmet criterion: ..." finding is about.
fn criterion_of(task: &Task, finding: &str) -> Option<usize> {
    criterion_index(task, finding.strip_prefix("Unmet criterion:")?)
}

/// The criterion a plain piece of text (a reviewer's ruling, the tail of an
/// "Unmet criterion:" finding) names.
pub(super) fn criterion_index(task: &Task, text: &str) -> Option<usize> {
    let rest = canon(text);
    if rest.is_empty() {
        return None;
    }
    task.criteria.iter().position(|c| {
        let head = canon(c.split("-- check:").next().unwrap_or(c));
        let full = canon(c);
        !head.is_empty()
            && (rest.starts_with(&head)
                || head.starts_with(&rest)
                || rest.contains(&head)
                || rest.contains(&full))
    })
}

/// A criterion this review marks unmet that an earlier attempt's review
/// marked unmet too and the judge has not ruled on yet: its index, the two
/// attempts and both reasons.
pub(super) struct Repeated {
    pub criterion: usize,
    pub earlier: (u32, String),
    pub current: (u32, String),
}

pub(super) fn repeated_unmet(
    task: &Task,
    idx: usize,
    attempt_n: u32,
    review: &ReviewResult,
) -> Option<Repeated> {
    for finding in &review.findings {
        let Some(i) = criterion_of(task, finding) else {
            continue;
        };
        if task.brief_check.judged.contains(&task.criteria[i]) {
            continue;
        }
        for earlier in task.attempts[..idx]
            .iter()
            .filter(|a| a.stage == Stage::Implement)
        {
            let Some(r) = earlier.review.as_ref() else {
                continue;
            };
            if let Some(f) = r.findings.iter().find(|f| criterion_of(task, f) == Some(i)) {
                return Some(Repeated {
                    criterion: i,
                    earlier: (earlier.n, strip_evidence(f).to_string()),
                    current: (attempt_n, strip_evidence(finding).to_string()),
                });
            }
        }
    }
    None
}

pub(super) fn ruling_prompt(task: &Task, rep: &Repeated) -> String {
    format!(
        "{RULING_PROMPT}\n\n{}\n",
        json!({
            "goal": task.goal,
            "criterion": task.criteria[rep.criterion],
            "reviews": [
                {"attempt": rep.earlier.0, "reason": rep.earlier.1},
                {"attempt": rep.current.0, "reason": rep.current.1},
            ],
        })
    )
}

pub(super) fn parse_ruling(text: &str) -> Option<Ruling> {
    let v = judge_json(text)?;
    let verdict = str_field(&v, "verdict").to_ascii_lowercase();
    if verdict.contains("infeasible") {
        let criterion = str_field(&v, "criterion");
        if criterion.is_empty() {
            return None;
        }
        return Some(Ruling::Infeasible {
            criterion,
            reason: reason_of(&v),
        });
    }
    verdict.contains("code").then_some(Ruling::CodeGap)
}

/// When the review marks a criterion unmet that an earlier review marked
/// unmet as well, a judge run decides between a code gap (the retry goes on
/// as before) and an infeasible criterion (amended, recorded, the owner told
/// without being blocked). `Ok(true)`: the criterion was amended, so the
/// same attempt is reviewed again against it.
pub(super) async fn heal_repeated_unmet(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    review: &ReviewResult,
    cancel: &CancelToken,
) -> Result<bool, RunError> {
    let attempt_n = task.attempts[idx].n;
    let Some(rep) = repeated_unmet(task, idx, attempt_n, review) else {
        return Ok(false);
    };
    let Some(route) = judge_route(app, &task.variant()) else {
        return Ok(false);
    };
    let old = task.criteria[rep.criterion].clone();
    let prompt = ruling_prompt(task, &rep);
    let reply = run_judge(
        app,
        task,
        &route,
        Some(attempt_n),
        "brief_check",
        "criterion-judge",
        &prompt,
        cancel,
    )
    .await?;
    let JudgeRun {
        reply,
        cost,
        timeout_note,
    } = reply;
    task.decisions.extend(timeout_note);
    task.cost_usd += cost;
    let ruling = match reply {
        Ok(text) => parse_ruling(&text),
        Err(_) => None,
    };
    // Whatever it said, one ruling per criterion.
    task.brief_check.judged.push(old.clone());
    let (a, b) = (rep.earlier.0, rep.current.0);
    match ruling {
        Some(Ruling::Infeasible { criterion, reason }) => {
            let settings = app.settings.read().unwrap().clone();
            if let Err(why) = amend_criterion(&settings, task, rep.criterion, &criterion) {
                task.decisions.push(format!(
                    "Orchestrator: criterion \"{old}\" stays as written: {why}"
                ));
                return Ok(false);
            }
            task.decisions.push(format!(
                "Orchestrator: criterion \"{old}\" was unmet in attempts {a} and {b} and is infeasible as written ({reason}); amended to \"{criterion}\""
            ));
            overturnable(
                task,
                format!("Can the criterion \"{old}\" be met as written?"),
                format!("No: amended to \"{criterion}\""),
                reason,
            );
            app.broadcast_log(
                &task.id,
                attempt_n,
                format!("Amended an infeasible criterion (the owner can overturn it): {criterion}"),
            );
            Ok(true)
        }
        Some(Ruling::CodeGap) => {
            task.decisions.push(format!(
                "Orchestrator: criterion \"{old}\" was unmet in attempts {a} and {b}; judged a code gap, retrying"
            ));
            Ok(false)
        }
        None => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// 3. Evidence
// ---------------------------------------------------------------------------

/// UI files (not tests) the task itself changed between an earlier attempt's
/// evidence tree and the worktree now. Files that differ only because the base
/// moved (carried-in commits, a conflict resolved onto a new base) are not the
/// task's doing; without the earlier base every difference counts.
pub(super) fn own_ui_changes_since(
    wt: &Path,
    old_tree: &str,
    old_base: Option<&str>,
    now_base: &str,
) -> Result<Vec<String>, git::GitError> {
    let now = git::worktree_tree(wt)?;
    let paths = match old_base {
        Some(old_base) => git::own_changes_between(wt, old_tree, old_base, &now, now_base)?,
        None => git::diff_trees_name_only(wt, old_tree, &now)?,
    };
    Ok(paths
        .into_iter()
        .filter(|p| is_ui_path(p) && !is_test_path(p))
        .collect())
}

/// Whether a repo-relative path is a UI file, by its extension or a UI-ish
/// directory name.
pub(super) fn is_ui_path(path: &str) -> bool {
    const EXT: [&str; 12] = [
        "tsx", "jsx", "vue", "svelte", "css", "scss", "sass", "less", "html", "htm", "svg", "xib",
    ];
    const DIRS: [&str; 8] = [
        "components",
        "ui",
        "views",
        "pages",
        "screens",
        "styles",
        "renderer",
        "frontend",
    ];
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    EXT.contains(&ext) || lower.split('/').any(|seg| DIRS.contains(&seg))
}

/// Runs the planner's screenshot command in the worktree and saves the
/// images the attempt left under `artifacts/`. A failing command is a
/// decision line, never a failed attempt.
pub(super) async fn capture_screenshots(
    app: &App,
    task: &mut Task,
    idx: usize,
    worktree: &Path,
    run_dir: &Path,
    sandbox: Option<&TaskSandbox>,
    cancel: &CancelToken,
) {
    let Some(cmd) = task.brief_check.screenshot.clone() else {
        return;
    };
    let ran = run_verify_commands(
        CheckContext::new(app, Some(&task.id)),
        worktree,
        &run_dir.join("screenshot"),
        std::slice::from_ref(&cmd),
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await;
    record_check_outcomes(app, task, &ran);
    if cancel.is_cancelled() {
        return;
    }
    if let Some(bad) = ran.iter().find(|r| r.code != Some(0)) {
        task.decisions.push(format!(
            "Orchestrator: screenshot command {cmd} exited {}; the images it left are used, the attempt is not failed for it",
            bad.code.map(|c| c.to_string()).unwrap_or_else(|| "null".into())
        ));
    }
    let since = task.attempts[idx].started_at;
    let saved = save_evidence(
        worktree,
        since,
        &run_dir.join("evidence"),
        &evidence_scope(task),
    );
    let a = &mut task.attempts[idx];
    for s in saved {
        if !a.evidence.contains(&s) {
            a.evidence.push(s);
        }
    }
    a.evidence.sort();
}

// ---------------------------------------------------------------------------
// 4. Review scope after the first attempt
// ---------------------------------------------------------------------------

fn is_path_like(token: &str) -> bool {
    let t = token.trim_matches(|c: char| {
        !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
    });
    let t = t.trim_end_matches('.');
    let t = match t.rsplit_once(':') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => t,
    };
    if t.contains('/') {
        return true;
    }
    match t.rsplit_once('.') {
        Some((name, ext)) => {
            name.len() >= 2
                && (2..=5).contains(&ext.len())
                && ext.chars().all(char::is_alphanumeric)
        }
        None => false,
    }
}

/// The files a finding names, and whether one of them is among `changed`.
fn names_changed_file(finding: &str, changed: &[String]) -> Option<bool> {
    let paths: Vec<&str> = finding
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | '`' | '\'' | '"'))
        .filter(|t| is_path_like(t))
        .collect();
    if paths.is_empty() {
        return None;
    }
    Some(paths.iter().any(|p| {
        let p = p.trim_matches(|c: char| c == ':' || c == '.');
        let p = match p.rsplit_once(':') {
            Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => head,
            _ => p,
        };
        changed
            .iter()
            .any(|f| f == p || f.ends_with(p) || p.ends_with(f.as_str()))
    }))
}

/// After the first attempt only a P0/P1 finding about the task's own work
/// blocks: an unmet criterion, a finding the reviewer says came back or an
/// earlier review reported, or one about a file the task changed (or naming
/// none). Everything else (P2/P3, a new P1 about files the task never
/// touched) is moved out of `review.findings` and returned as follow-ups; a
/// review left with no blocking finding becomes a PASS.
pub(super) fn scope_review(
    task: &Task,
    idx: usize,
    changed: &[String],
    review: &mut ReviewResult,
) -> Vec<String> {
    let earlier: Vec<String> = task.attempts[..idx]
        .iter()
        .filter_map(|a| a.review.as_ref())
        .flat_map(|r| r.findings.iter().map(|f| canon(f)))
        .collect();
    let mut keep_findings = Vec::new();
    let mut keep_severities = Vec::new();
    let mut follow_ups = Vec::new();
    for (i, finding) in review.findings.iter().enumerate() {
        let severity = review
            .severities
            .get(i)
            .copied()
            .flatten()
            .or_else(|| brief::severity_of(finding));
        let blocks = if finding.starts_with("Unmet criterion:")
            || finding.starts_with("Another attempt was asked for:")
        {
            true
        } else if finding.starts_with("Not checked by review:") {
            false
        } else {
            match severity {
                Some(0) => true,
                Some(1) | None => {
                    review.repeated.contains(finding)
                        || earlier.contains(&canon(finding))
                        || names_changed_file(finding, changed) != Some(false)
                }
                Some(_) => false,
            }
        };
        if blocks {
            keep_findings.push(finding.clone());
            keep_severities.push(severity);
        } else {
            follow_ups.push(finding.clone());
        }
    }
    review.findings = keep_findings;
    review.severities = keep_severities;
    if review.verdict == Verdict::Fail && review.findings.is_empty() {
        review.verdict = Verdict::Pass;
    }
    follow_ups
}

/// Adds follow-ups to the task, once each, up to a cap.
pub(super) fn add_follow_ups(task: &mut Task, findings: Vec<String>) {
    for f in findings {
        let f = truncate_chars(f.trim(), 400);
        if !f.is_empty()
            && !task.brief_check.follow_ups.contains(&f)
            && task.brief_check.follow_ups.len() < MAX_FOLLOW_UPS
        {
            task.brief_check.follow_ups.push(f);
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Owner answers against the criteria
// ---------------------------------------------------------------------------

const ANSWER_PROMPT: &str = "You judge whether the owner's answers to a coding task's questions contradict any of its acceptance criteria. An answer contradicts a criterion when following the answer makes the criterion, as written, wrong or unmeetable. An answer that only adds detail, or that a criterion already agrees with, does not. Reply with only JSON: {\"amend\": [{\"criterion\": <index>, \"text\": \"<the criterion rewritten to agree with the answer, keeping its -- check: part>\"}], \"reason\": \"<one sentence>\"}. Use an empty list when nothing contradicts.";

/// The decisions after the first `seen` that record an owner, lead or policy answer.
pub(super) fn new_answers(task: &Task) -> Vec<String> {
    task.decisions
        .iter()
        .skip(task.brief_check.answers_seen)
        .filter(|d| {
            (d.starts_with("Owner: ")
                && !d.starts_with("Owner: stop")
                && !d.starts_with("Owner: dropped"))
                || (d.starts_with("Lead: ") && d.contains(" -> "))
                || d.starts_with("Policy: ")
        })
        .cloned()
        .collect()
}

pub(super) fn answer_prompt(task: &Task, answers: &[String]) -> String {
    let criteria: Vec<String> = task
        .criteria
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{i}: {c}"))
        .collect();
    format!(
        "{ANSWER_PROMPT}\n\n{}\n",
        json!({"goal": task.goal, "criteria": criteria, "answers": answers})
    )
}

pub(super) fn parse_amendments(text: &str, criteria: usize) -> Vec<(usize, String)> {
    let Some(v) = judge_json(text) else {
        return Vec::new();
    };
    v.get("amend")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let i = item.get("criterion")?.as_u64()? as usize;
            let text = str_field(item, "text");
            (i < criteria && !text.is_empty()).then_some((i, text))
        })
        .collect()
}

/// Reads the owner (and policy) answers recorded since the last look against
/// the criteria; a criterion an answer contradicts is amended with a
/// decision line and an assumption, so the next review checks what the owner
/// decided. Returns whether any criterion changed.
pub(super) async fn heal_answers(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    cancel: &CancelToken,
) -> Result<bool, RunError> {
    let answers = new_answers(task);
    let seen = task.decisions.len();
    if answers.is_empty() || task.criteria.is_empty() {
        task.brief_check.answers_seen = seen;
        return Ok(false);
    }
    let Some(route) = judge_route(app, &task.variant()) else {
        // Not looked at: a later run with the judge on still reads them.
        return Ok(false);
    };
    let prompt = answer_prompt(task, &answers);
    let reply = run_judge(
        app,
        task,
        &route,
        Some(attempt_n),
        "brief_check",
        "answer-check",
        &prompt,
        cancel,
    )
    .await?;
    let JudgeRun {
        reply,
        cost,
        timeout_note,
    } = reply;
    task.decisions.extend(timeout_note);
    task.cost_usd += cost;
    let amendments = match reply {
        Ok(text) => parse_amendments(&text, task.criteria.len()),
        Err(_) => Vec::new(),
    };
    let mut changed = false;
    for (i, text) in amendments {
        let old = task.criteria[i].clone();
        if old == text {
            continue;
        }
        let settings = app.settings.read().unwrap().clone();
        if let Err(why) = amend_criterion(&settings, task, i, &text) {
            task.decisions.push(format!(
                "Orchestrator: criterion \"{old}\" stays as written: {why}"
            ));
            continue;
        }
        task.decisions.push(format!(
            "Orchestrator: criterion \"{old}\" contradicts the owner's answer; amended to \"{text}\""
        ));
        overturnable(
            task,
            format!("Does the criterion \"{old}\" still hold after the owner's answer?"),
            format!("No: amended to \"{text}\""),
            truncate_chars(&answers.join(" | "), MAX_REASON_CHARS),
        );
        changed = true;
    }
    task.brief_check.answers_seen = task.decisions.len();
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_rewritten_verify_starts_the_preflight_again_for_the_new_commands() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.verify = vec!["echo one".into()];
        let cancel = CancelToken::new();
        let job = |app: &Arc<App>| app.base_preflights.lock().unwrap()[&task.id].clone();
        spawn_base_preflight(&app, &task, &cancel);
        let first = job(&app);
        // The same commands again: the job is not started twice.
        spawn_base_preflight(&app, &task, &cancel);
        assert!(Arc::ptr_eq(&first, &job(&app)));
        // A rewrite replaces the job and stops the old one.
        task.verify = vec!["echo two".into()];
        spawn_base_preflight(&app, &task, &cancel);
        let second = job(&app);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second.commands, vec!["echo two".to_string()]);
        assert!(first.cancel.is_cancelled());
        assert!(!second.cancel.is_cancelled());
    }

    #[test]
    fn grep_patterns_starting_with_a_dash_get_a_double_dash() {
        assert_eq!(
            fix_grep_dashes("grep -qF '- `formatDuration` x' README.md").unwrap(),
            "grep -qF -- '- `formatDuration` x' README.md"
        );
        assert_eq!(
            fix_grep_dashes("test -f a && grep -q \"-x\" a | grep -c -v '-y' b").unwrap(),
            "test -f a && grep -q -- \"-x\" a | grep -c -v -- '-y' b"
        );
        for fine in [
            "grep -qF -- '- x' README.md",
            "grep -qF 'x' README.md",
            "grep -e '-x' a",
            "grep -qe '-x' a",
            "echo grep '-x'",
            "grep -rn foo src",
            "grep -rn --include='*.ts' foo src",
            "grep --regexp='-x' src",
            "grep --include='*.ts' --regexp=foo a",
        ] {
            assert_eq!(fix_grep_dashes(fine), None, "{fine}");
        }
    }

    #[test]
    fn a_pattern_after_quoted_long_options_still_gets_the_double_dash() {
        assert_eq!(
            fix_grep_dashes("grep -rn --include='*.ts' '- x' src").unwrap(),
            "grep -rn --include='*.ts' -- '- x' src"
        );
    }

    #[test]
    fn a_broken_grep_check_is_fixed_everywhere_with_one_decision_line() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        task.verify = vec!["grep -qF '- x' R.md".into(), "npm test".into()];
        task.final_verify = vec!["grep -qF '- x' R.md".into()];
        task.decisions.clear();
        assert!(heal_grep_dashes(&mut task));
        assert_eq!(task.verify, ["grep -qF -- '- x' R.md", "npm test"]);
        assert_eq!(task.final_verify, ["grep -qF -- '- x' R.md"]);
        assert_eq!(task.decisions.len(), 1);
        assert!(!heal_grep_dashes(&mut task));
    }

    #[test]
    fn ui_paths_are_told_by_extension_or_directory() {
        assert!(is_ui_path("src/App.tsx"));
        assert!(is_ui_path("web/styles/main.css"));
        assert!(is_ui_path("app/components/x.js"));
        assert!(!is_ui_path("orchd/src/engine/plan.rs"));
        assert!(!is_ui_path("README.md"));
    }

    #[test]
    fn a_finding_names_the_files_it_is_about() {
        let changed = vec!["src/a.rs".to_string()];
        assert_eq!(
            names_changed_file("P1: src/a.rs:12 - bug", &changed),
            Some(true)
        );
        assert_eq!(
            names_changed_file("P1: other/b.rs:3 - bug", &changed),
            Some(false)
        );
        assert_eq!(
            names_changed_file("P1: something is wrong, e.g. here", &changed),
            None
        );
    }

    #[test]
    fn feasibility_replies_are_read_leniently() {
        assert!(matches!(
            parse_feasibility("{\"verdict\":\"feasible\"}", 1),
            Some(Feasibility::AsWritten)
        ));
        assert!(matches!(
            parse_feasibility("x {\"verdict\":\"rewrite\",\"command\":\"true\"} y", 1),
            Some(Feasibility::Rewrite { .. })
        ));
        assert!(parse_feasibility("{\"verdict\":\"rewrite\"}", 1).is_none());
        assert!(parse_feasibility("nothing", 1).is_none());
    }

    #[test]
    fn a_ruling_names_infeasible_or_code_gap() {
        assert!(matches!(
            parse_ruling("{\"verdict\":\"infeasible as written\",\"criterion\":\"c\"}"),
            Some(Ruling::Infeasible { .. })
        ));
        assert!(matches!(
            parse_ruling("{\"verdict\":\"code gap\"}"),
            Some(Ruling::CodeGap)
        ));
        assert!(parse_ruling("{\"verdict\":\"infeasible as written\"}").is_none());
    }

    #[test]
    fn amendments_outside_the_criteria_are_dropped() {
        let text = "{\"amend\":[{\"criterion\":0,\"text\":\"a\"},{\"criterion\":5,\"text\":\"b\"},{\"criterion\":1,\"text\":\"\"}]}";
        assert_eq!(parse_amendments(text, 2), vec![(0, "a".to_string())]);
    }

    fn evidence_repo() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        impl Fn(&[&str]) -> String,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let dir = repo.clone();
        let git = move |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "hi\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        (tmp, repo, git)
    }

    #[test]
    fn evidence_survives_carried_in_base_commits_but_not_the_tasks_own_ui_edit() {
        let (_tmp, repo, git) = evidence_repo();
        let base1 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "task"]);
        std::fs::write(repo.join("Button.tsx"), "v1\n").unwrap();
        let old_tree = git::worktree_tree(&repo).unwrap();
        git(&["checkout", "-q", "-b", "other", &base1]);
        std::fs::create_dir_all(repo.join("orchd")).unwrap();
        std::fs::write(repo.join("Landed.tsx"), "l\n").unwrap();
        std::fs::write(repo.join("orchd/x.rs"), "r\n").unwrap();
        git(&["add", "Landed.tsx", "orchd"]);
        git(&["commit", "-q", "-m", "landed"]);
        let base2 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "task"]);
        std::fs::write(repo.join("Landed.tsx"), "l\n").unwrap();
        std::fs::create_dir_all(repo.join("orchd")).unwrap();
        std::fs::write(repo.join("orchd/x.rs"), "r\n").unwrap();
        // Only carried-in commits changed files: reuse.
        assert!(own_ui_changes_since(&repo, &old_tree, Some(&base1), &base2)
            .unwrap()
            .is_empty());
        // Without the earlier base the carried-in file looks like a change.
        assert_eq!(
            own_ui_changes_since(&repo, &old_tree, None, &base2).unwrap(),
            vec!["Landed.tsx".to_string()]
        );
        // The task edits its own UI file: new evidence needed.
        std::fs::write(repo.join("Button.tsx"), "v2\n").unwrap();
        assert_eq!(
            own_ui_changes_since(&repo, &old_tree, Some(&base1), &base2).unwrap(),
            vec!["Button.tsx".to_string()]
        );
    }

    #[test]
    fn a_conflict_resolution_outside_the_ui_keeps_the_evidence() {
        let (_tmp, repo, git) = evidence_repo();
        let base1 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "task"]);
        std::fs::write(repo.join("Button.tsx"), "v1\n").unwrap();
        std::fs::write(repo.join("notes.md"), "mine\n").unwrap();
        let old_tree = git::worktree_tree(&repo).unwrap();
        git(&["checkout", "-q", "-b", "other", &base1]);
        std::fs::write(repo.join("notes.md"), "theirs\n").unwrap();
        git(&["add", "notes.md"]);
        git(&["commit", "-q", "-m", "landed"]);
        let base2 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "task"]);
        std::fs::write(repo.join("notes.md"), "merged\n").unwrap();
        assert!(own_ui_changes_since(&repo, &old_tree, Some(&base1), &base2)
            .unwrap()
            .is_empty());
        // A resolution that rewrites a UI file needs fresh evidence.
        std::fs::write(repo.join("Button.tsx"), "resolved\n").unwrap();
        assert_eq!(
            own_ui_changes_since(&repo, &old_tree, Some(&base1), &base2).unwrap(),
            vec!["Button.tsx".to_string()]
        );
    }
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    #[test]
    fn new_answers_feed_lead_answers_but_not_lead_stops_to_the_criteria_check() {
        let mut task = task_with_status(TaskStatus::Queued);
        task.decisions = vec![
            "Lead: old question -> old answer (answered by a lead session)".into(),
            "Lead: q -> a (answered by a lead session)".into(),
            "Lead: stop the service? -> yes (answered by a lead session)".into(),
            "Lead: stop".into(),
            "Lead: dropped from parent".into(),
            "Owner: B".into(),
            "Policy: q -> a (policy: e)".into(),
        ];
        task.brief_check.answers_seen = 1;
        assert_eq!(
            new_answers(&task),
            [
                task.decisions[1].clone(),
                task.decisions[2].clone(),
                task.decisions[5].clone(),
                task.decisions[6].clone()
            ]
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn feasibility_timed_out_keeps_the_command_unchanged() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.verify = vec!["false".into()];
        let result = run_base_preflight(&app, &task, &CancelToken::new())
            .await
            .unwrap();
        assert!(result.settled);
        assert!(result.rulings.is_empty());
        task.decisions.extend(result.decisions);
        assert_eq!(task.verify, ["false"]);
        assert_helper_timeout(&task, "feasibility");
        let costs = crate::costs::read_all(&app.data_dir);
        assert_eq!(costs.len(), 1);
        assert!(costs[0].attempt.is_none());
    }

    #[tokio::test]
    async fn criterion_judge_timed_out_keeps_the_criterion() {
        let (app, _dir, mut task, _) = slow_helper_task();
        let review: ReviewResult = serde_json::from_value(
            json!({"verdict":"FAIL", "findings":["Unmet criterion: Keep the API (missing)"]}),
        )
        .unwrap();
        task.attempts[0].review = Some(review.clone());
        task.attempts.push(attempt_with_failure(2, "failure"));
        let criteria = task.criteria.clone();
        assert!(matches!(
            heal_repeated_unmet(&app, &mut task, 1, &review, &CancelToken::new()).await,
            Ok(false)
        ));
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "criterion-judge");
    }

    #[tokio::test]
    async fn answer_check_timed_out_keeps_the_criteria() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.decisions.push("Owner: Which API? -> Remove it".into());
        let criteria = task.criteria.clone();
        assert!(matches!(
            heal_answers(&app, &mut task, 1, &CancelToken::new()).await,
            Ok(false)
        ));
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "answer-check");
    }
}
