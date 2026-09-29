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
pub(super) fn judge_route(app: &App) -> Option<Route> {
    check_route(app)?.1
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
        overturned: false,
        owner_answer: None,
    });
}

/// Replaces criterion `i` with `text`, keeping its visual mark.
fn amend_criterion(task: &mut Task, i: usize, text: &str) {
    let old = std::mem::replace(&mut task.criteria[i], text.to_string());
    for v in task.visual_criteria.iter_mut().filter(|v| **v == old) {
        *v = text.to_string();
    }
}

// ---------------------------------------------------------------------------
// 1. Feasibility of a check on the base
// ---------------------------------------------------------------------------

const FEASIBILITY_PROMPT: &str = "You judge whether a check command of a coding task can work in this repository. The command was run on the repository as it is before any work and exited non-zero. You may read files in the working directory but change nothing.\n\nAnswer with one verdict:\n- `feasible`: it fails as it should, because the task is meant to make it pass (for example a test for the new behaviour).\n- `rewrite`: the repository cannot run this kind of check (the test runner, harness, script or tool it needs does not exist here). Give `command`, the nearest check this repository can really run and that exits 0 only when the criterion holds, and `criterionText` when the criterion's own `-- check:` wording must change with it.\n- `unrelated`: it fails for a reason that has nothing to do with the task (a broken environment, a flaky or already broken test, a missing credential).\n\nReply with only JSON: {\"verdict\": \"feasible|rewrite|unrelated\", \"command\": \"\", \"criterion\": <index or null>, \"criterionText\": \"\", \"reason\": \"<one sentence>\"}";

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

/// Applies a ruling about `command`; `false` when it changed nothing.
pub(super) fn apply_feasibility(task: &mut Task, command: &str, ruling: Feasibility) -> bool {
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
                amend_criterion(task, i, &text);
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

/// Runs every check command of the task on its base once (`base_check`);
/// each one that fails there goes to the judge, which leaves it (the task is
/// meant to make it pass), rewrites it to a check the repository can run, or
/// marks it non-gating. Settles for good, once per task. `false`: cancelled.
pub(super) async fn heal_feasibility(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    cancel: &CancelToken,
) -> bool {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return true;
    };
    if task.brief_check.feasibility_done {
        return true;
    }
    let Some(route) = judge_route(app) else {
        return true;
    };
    let mut commands: Vec<String> = Vec::new();
    for c in task
        .verify
        .iter()
        .chain(task.final_verify.iter())
        .chain(task.checks.iter().map(|c| &c.run))
    {
        if !c.trim().is_empty() && !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    let run_dir = app
        .store
        .run_dir(task_id, attempt_n)
        .join("feasibility-base");
    let ran = run_on_base(app, &task.repo, &task.base_sha, &commands, &run_dir, cancel).await;
    if cancel.is_cancelled() {
        return false;
    }
    // A base that could not be checked out says nothing: try again later.
    let Ok(results) = ran else {
        return true;
    };
    let mut rulings: Vec<(VerifyOutcome, Feasibility)> = Vec::new();
    for (i, failed) in results.iter().filter(|r| r.code != Some(0)).enumerate() {
        let Ok(Some(current)) = app.store.load_task(task_id) else {
            return true;
        };
        let prompt = feasibility_prompt(&current, failed);
        let name = format!("feasibility-{i}");
        let reply = run_judge(
            app,
            &current,
            &route,
            attempt_n,
            "brief_check",
            &name,
            &prompt,
            cancel,
        )
        .await;
        match reply {
            Err(RunError::Cancelled) => return false,
            Ok((reply, cost)) => {
                add_task_cost(app, task_id, cost);
                if let Some(r) = reply
                    .ok()
                    .and_then(|text| parse_feasibility(&text, current.criteria.len()))
                {
                    rulings.push((failed.clone(), r));
                }
            }
            Err(RunError::Io(_)) => {}
        }
    }
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return true;
    };
    let mut changed = false;
    for (failed, ruling) in rulings {
        changed |= apply_feasibility(&mut task, &failed.command, ruling);
    }
    if changed {
        app.verify_cache.lock().unwrap().remove(task_id);
    }
    task.brief_check.feasibility_done = true;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
    true
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
    let rest = canon(finding.strip_prefix("Unmet criterion:")?);
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
    let Some(route) = judge_route(app) else {
        return Ok(false);
    };
    let old = task.criteria[rep.criterion].clone();
    let prompt = ruling_prompt(task, &rep);
    let name = format!("criterion-judge-{}", task.brief_check.judged.len());
    let reply = run_judge(
        app,
        task,
        &route,
        attempt_n,
        "brief_check",
        &name,
        &prompt,
        cancel,
    )
    .await?;
    let (reply, cost) = reply;
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
            amend_criterion(task, rep.criterion, &criterion);
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
    task: &mut Task,
    idx: usize,
    worktree: &Path,
    run_dir: &Path,
    sandbox: SandboxMode,
    cancel: &CancelToken,
) {
    let Some(cmd) = task.brief_check.screenshot.clone() else {
        return;
    };
    let ran = run_verify_commands(
        worktree,
        &run_dir.join("screenshot"),
        std::slice::from_ref(&cmd),
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await;
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
    let saved = save_evidence(worktree, since, &run_dir.join("evidence"));
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

/// The decisions after the first `seen` that record an owner or policy answer.
pub(super) fn new_answers(task: &Task) -> Vec<String> {
    task.decisions
        .iter()
        .skip(task.brief_check.answers_seen)
        .filter(|d| {
            (d.starts_with("Owner: ")
                && !d.starts_with("Owner: stop")
                && !d.starts_with("Owner: dropped"))
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
    let Some(route) = judge_route(app) else {
        // Not looked at: a later run with the judge on still reads them.
        return Ok(false);
    };
    let prompt = answer_prompt(task, &answers);
    let name = format!("answer-check-{seen}");
    let reply = run_judge(
        app,
        task,
        &route,
        attempt_n,
        "brief_check",
        &name,
        &prompt,
        cancel,
    )
    .await?;
    let (reply, cost) = reply;
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
        amend_criterion(task, i, &text);
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
}
