use super::*;

/// Review runs one implement attempt may have: the first and one more,
/// whatever asks for it (no verdict, a judge drop, a healed criterion, the
/// owner's `retry`).
pub(super) const MAX_REVIEW_RUNS: u32 = 2;

/// The decision line for a review loop that ran out of runs.
pub(super) fn review_stopped_note(attempt_n: u32) -> String {
    format!("Orchestrator: attempt {attempt_n} review stopped after {MAX_REVIEW_RUNS} runs")
}

/// Reviews of a finding after the first that make it a repeat to handle.
const HANDLE_AT_REPEATS: u32 = 2;

const ID_OPEN: &str = " [F-";

/// `text` without the ` [F-xxxxxx]` mark `with_id` appended.
fn strip_finding_id(text: &str) -> &str {
    let t = text.trim_end();
    if let Some(start) = t.rfind(ID_OPEN) {
        let tail = &t[start + ID_OPEN.len()..];
        if tail.len() == 7
            && tail.ends_with(']')
            && tail[..6].chars().all(|c| c.is_ascii_hexdigit())
        {
            return t[..start].trim_end();
        }
    }
    t
}

/// The id a finding keeps for its whole life: `F-` and the first six hex
/// characters of the FNV-1a hash of its key.
pub(super) fn finding_id(text: &str) -> String {
    let key = finding_key(strip_finding_id(text));
    format!("F-{}", &crate::harness::short_hash(&[&key])[..6])
}

fn with_id(text: &str, id: &str) -> String {
    format!("{} [{id}]", strip_finding_id(text))
}

/// The id a finding text carries, when it carries one.
fn id_of(text: &str) -> Option<String> {
    let t = text.trim_end();
    let stripped = strip_finding_id(text);
    (stripped.len() != t.len()).then(|| format!("F-{}", &t[t.len() - 7..t.len() - 1]))
}

/// Removes the findings at `drop` (indexes into `r.findings`) with their
/// parallel marks. A FAIL left with no finding becomes a PASS.
fn remove_findings(r: &mut ReviewResult, drop: &[usize]) {
    if drop.is_empty() {
        return;
    }
    let gone: Vec<String> = drop.iter().map(|i| r.findings[*i].clone()).collect();
    let keep = |i: usize| !drop.contains(&i);
    let mut n = 0;
    r.findings.retain(|_| {
        n += 1;
        keep(n - 1)
    });
    let mut n = 0;
    r.severities.retain(|_| {
        n += 1;
        keep(n - 1)
    });
    let mut n = 0;
    r.details.retain(|_| {
        n += 1;
        keep(n - 1)
    });
    r.repeated.retain(|t| !gone.contains(t));
    if r.verdict == Verdict::Fail && r.findings.is_empty() {
        r.verdict = Verdict::Pass;
    }
}

/// Moves the findings of kind `style` or `nit` out of the review into the
/// task's follow-ups: they never block, on any attempt.
pub(super) fn move_minor_findings(task: &mut Task, attempt_n: u32, r: &mut ReviewResult) {
    let minor: Vec<usize> = (0..r.findings.len())
        .filter(|i| {
            r.details
                .get(*i)
                .and_then(|d| d.kind.as_deref())
                .is_some_and(|k| matches!(k, "style" | "nit"))
        })
        .collect();
    if minor.is_empty() {
        return;
    }
    let moved: Vec<String> = minor.iter().map(|i| r.findings[*i].clone()).collect();
    remove_findings(r, &minor);
    task.decisions.push(format!(
        "Orchestrator: attempt {attempt_n}'s review: {} style or nit finding(s) are follow-ups, not blockers",
        moved.len()
    ));
    add_follow_ups(task, moved);
}

/// Puts the reviewer's own findings of one review into the task's registry
/// and gives each its id in the review's text. A finding the registry holds
/// as resolved and that comes back without a new failing scenario is dropped.
pub(super) fn register_findings(task: &mut Task, attempt_n: u32, round: u32, r: &mut ReviewResult) {
    let own = r.severities.len().min(r.findings.len());
    let mut present: Vec<String> = Vec::new();
    let mut dropped: Vec<usize> = Vec::new();
    let mut renamed: Vec<(String, String)> = Vec::new();
    for i in 0..own {
        let raw = strip_finding_id(&r.findings[i]).to_string();
        let id = finding_id(&raw);
        let run = r
            .details
            .get(i)
            .and_then(|d| d.scenario.as_ref())
            .map(|s| s.run.clone());
        match task.findings.iter_mut().find(|f| f.id == id) {
            Some(rec) if rec.status == FindingStatus::Resolved => {
                let new_run = run.as_ref().filter(|run| !rec.scenarios.contains(run));
                let Some(new_run) = new_run else {
                    task.decisions.push(format!(
                        "Orchestrator: review finding {id} came back after it was resolved, with no new failing scenario; dropped: {raw}"
                    ));
                    dropped.push(i);
                    continue;
                };
                rec.scenarios.push(new_run.clone());
                rec.status = FindingStatus::Open;
                rec.attempt = attempt_n;
                rec.round = round;
                task.decisions.push(format!(
                    "Orchestrator: review finding {id} came back with a new failing scenario ({new_run}); kept"
                ));
            }
            Some(rec) => {
                if rec.attempt < attempt_n {
                    rec.repeats += 1;
                }
                rec.attempt = attempt_n;
                rec.round = round;
                if let Some(run) = run.filter(|run| !rec.scenarios.contains(run)) {
                    rec.scenarios.push(run);
                }
            }
            None => task.findings.push(FindingRecord {
                id: id.clone(),
                text: raw.clone(),
                status: FindingStatus::Open,
                attempt: attempt_n,
                round,
                repeats: 0,
                scenarios: run.into_iter().collect(),
                handled: false,
            }),
        }
        present.push(id.clone());
        renamed.push((r.findings[i].clone(), with_id(&raw, &id)));
    }
    for rec in task.findings.iter_mut() {
        if rec.status == FindingStatus::Open && !present.contains(&rec.id) {
            rec.status = FindingStatus::Resolved;
        }
    }
    for (old, new) in renamed {
        for t in r.findings.iter_mut().chain(r.repeated.iter_mut()) {
            if *t == old {
                *t = new.clone();
            }
        }
    }
    remove_findings(r, &dropped);
}

/// What the loop does about the findings that repeated for the second time:
/// a failing scenario becomes a criterion and a check; every other one goes
/// to the finding judge (with the implementer's report as the rebuttal when
/// the attempt disputed nothing). They no longer count as repeated, so the
/// next step is another attempt, not an owner question. Returns the
/// (finding, why) pairs the judge ruled invalid.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_repeated_findings(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    r: &mut ReviewResult,
    implementer_note: &str,
    settings: &Settings,
    review_route: &Route,
    implementer: &Route,
    judge_runs: &mut u32,
    worktree: &Path,
    cancel: &CancelToken,
) -> Vec<(String, String)> {
    let own = r.severities.len().min(r.findings.len());
    let mut to_judge: Vec<String> = Vec::new();
    let mut made_checks = false;
    for i in 0..own {
        let finding = r.findings[i].clone();
        let Some(id) = id_of(&finding) else { continue };
        let Some(pos) = task.findings.iter().position(|f| f.id == id) else {
            continue;
        };
        if task.findings[pos].handled {
            r.repeated.retain(|t| *t != finding);
            continue;
        }
        if task.findings[pos].repeats < HANDLE_AT_REPEATS {
            continue;
        }
        task.findings[pos].handled = true;
        r.repeated.retain(|t| *t != finding);
        let run = r
            .details
            .get(i)
            .and_then(|d| d.scenario.as_ref())
            .map(|s| s.run.clone())
            .or_else(|| task.findings[pos].scenarios.last().cloned())
            .filter(|run| !run.trim().is_empty());
        if let Some(run) = run {
            let dir = app
                .store
                .run_dir(&task.id, attempt_n)
                .join(format!("finding-scenario-{id}"));
            let _ = std::fs::create_dir_all(&dir);
            let results = run_verify_commands(
                CheckContext::new(app, Some(&task.id)),
                worktree,
                &dir,
                std::slice::from_ref(&run),
                settings.sandbox,
                &task.base_sha,
                cancel,
            )
            .await;
            if cancel.is_cancelled() {
                break;
            }
            if results.first().is_some_and(|v| v.code != Some(0)) {
                task.criteria
                    .push(format!("Review finding {id} is fixed -- check: {run}"));
                task.checks.push(Check {
                    criterion: task.criteria.len() - 1,
                    run: run.clone(),
                    baseline: Some(Baseline::Fail),
                });
                task.decisions.push(format!(
                    "Orchestrator: review finding {id} repeated twice and its scenario fails in the worktree; added a criterion and a check: {run}"
                ));
                made_checks = true;
                continue;
            }
            task.decisions.push(format!(
                "Orchestrator: review finding {id} repeated twice; its scenario passes in the worktree, so the judge rules on it"
            ));
        } else {
            task.decisions.push(format!(
                "Orchestrator: review finding {id} repeated twice with no scenario; the judge rules on it"
            ));
        }
        to_judge.push(finding);
    }
    if made_checks || !to_judge.is_empty() {
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
    }
    if to_judge.is_empty() || cancel.is_cancelled() {
        return Vec::new();
    }
    judge_disputed_findings(
        app,
        task,
        idx,
        attempt_n,
        &to_judge,
        Some(&truncate_chars(implementer_note.trim(), 2000)),
        settings,
        review_route,
        implementer,
        task.tier,
        judge_runs,
        cancel,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(findings: &[&str]) -> ReviewResult {
        ReviewResult {
            verdict: Verdict::Fail,
            findings: findings.iter().map(|s| s.to_string()).collect(),
            repeated: Vec::new(),
            severities: findings.iter().map(|_| Some(1)).collect(),
            details: Vec::new(),
            criteria: Vec::new(),
        }
    }

    #[test]
    fn an_id_is_stable_and_ignores_its_own_mark() {
        let id = finding_id("P1: src/a.rs:10 - the parser drops the last row");
        assert!(id.starts_with("F-") && id.len() == 8, "{id}");
        assert_eq!(
            id,
            finding_id("P1: src/a.rs:44 - the parser drops the last row")
        );
        let marked = with_id("P1: the parser drops the last row", &id);
        assert_eq!(id_of(&marked).as_deref(), Some(id.as_str()));
        assert_eq!(
            finding_id(&marked),
            finding_id("P1: the parser drops the last row")
        );
    }

    #[test]
    fn a_registry_entry_flips_to_resolved_when_a_review_omits_it() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut r = review(&["P1: first problem here"]);
        register_findings(&mut task, 1, 1, &mut r);
        assert_eq!(task.findings.len(), 1);
        assert_eq!(task.findings[0].status, FindingStatus::Open);
        assert!(r.findings[0].ends_with(&format!("[{}]", task.findings[0].id)));
        let mut next = review(&["P1: other problem entirely"]);
        register_findings(&mut task, 2, 1, &mut next);
        assert_eq!(task.findings[0].status, FindingStatus::Resolved);
        assert_eq!(task.findings.len(), 2);
    }
}
