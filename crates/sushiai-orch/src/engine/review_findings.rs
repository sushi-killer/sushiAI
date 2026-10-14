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

/// `text` without the ` [F-id]` mark `with_id` appended.
fn strip_finding_id(text: &str) -> &str {
    let t = text.trim_end();
    if let Some(start) = t.rfind(" [F-") {
        if let Some(id) = t[start + 2..].strip_suffix(']') {
            if id.len() > 2 && id[2..].chars().all(|c| c.is_ascii_alphanumeric()) {
                return t[..start].trim_end();
            }
        }
    }
    t
}

fn with_id(text: &str, id: &str) -> String {
    format!("{} [{id}]", strip_finding_id(text))
}

/// The first `F-<n>` no registered finding has.
fn new_finding_id(task: &Task) -> String {
    (task.findings.len() + 1..)
        .map(|n| format!("F-{n}"))
        .find(|id| task.findings.iter().all(|f| f.id != *id))
        .expect("an unused id")
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
/// and gives each its id: the registered finding it named, else a new one.
/// The id is in the finding's `details` and marked in its text. A finding
/// the registry holds as resolved and that comes back is kept as a repeat
/// that is handled at once: its scenario is rerun, or the judge rules on it.
/// A finding open since an earlier review run, or
/// reopened, is a repeat.
pub(super) fn register_findings(task: &mut Task, attempt_n: u32, round: u32, r: &mut ReviewResult) {
    let own = r.severities.len().min(r.findings.len());
    if r.details.len() < own {
        r.details.resize(own, Default::default());
    }
    r.repeated.clear();
    let mut present: Vec<String> = Vec::new();
    for i in 0..own {
        let raw = strip_finding_id(&r.findings[i]).to_string();
        let run = r.details[i].scenario.as_ref().map(|s| s.run.clone());
        let named = r.details[i].id.as_deref().and_then(|id| {
            let pos = task.findings.iter().position(|f| f.id == id)?;
            (!present.contains(&task.findings[pos].id)).then_some(pos)
        });
        let mut repeat = false;
        let id = match named {
            Some(pos) if task.findings[pos].status == FindingStatus::Resolved => {
                let rec = &mut task.findings[pos];
                let id = rec.id.clone();
                if let Some(run) = run.filter(|run| !rec.scenarios.contains(run)) {
                    rec.scenarios.push(run);
                }
                rec.status = FindingStatus::Open;
                rec.attempt = attempt_n;
                rec.round = round;
                rec.repeats = rec.repeats.max(HANDLE_AT_REPEATS);
                rec.handled = false;
                repeat = true;
                task.decisions.push(format!(
                    "Orchestrator: review finding {id} came back after it was resolved; kept, its scenario or the judge rules on it"
                ));
                id
            }
            Some(pos) => {
                let rec = &mut task.findings[pos];
                repeat = rec.attempt < attempt_n || rec.round < round;
                if rec.attempt < attempt_n {
                    rec.repeats += 1;
                }
                rec.attempt = attempt_n;
                rec.round = round;
                if let Some(run) = run.filter(|run| !rec.scenarios.contains(run)) {
                    rec.scenarios.push(run);
                }
                rec.id.clone()
            }
            None => {
                let id = new_finding_id(task);
                task.findings.push(FindingRecord {
                    id: id.clone(),
                    text: raw.clone(),
                    status: FindingStatus::Open,
                    attempt: attempt_n,
                    round,
                    repeats: 0,
                    scenarios: run.into_iter().collect(),
                    handled: false,
                });
                id
            }
        };
        r.details[i].id = Some(id.clone());
        present.push(id.clone());
        r.findings[i] = with_id(&raw, &id);
        if repeat {
            r.repeated.push(r.findings[i].clone());
        }
    }
    for rec in task.findings.iter_mut() {
        if rec.status == FindingStatus::Open && !present.contains(&rec.id) {
            rec.status = FindingStatus::Resolved;
        }
    }
}

/// What the loop does about the findings that repeated for the second time:
/// a failing scenario becomes a criterion and a check. Every other one stays
/// open and blocking: the implementer may decline it with evidence, and the
/// next review rules on that (`apply_declines`). They no longer count as
/// repeated, so the next step is another attempt, not an owner question.
pub(super) async fn handle_repeated_findings(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    r: &mut ReviewResult,
    worktree: &Path,
    cancel: &CancelToken,
) {
    let own = r.severities.len().min(r.findings.len());
    let mut changed = false;
    for i in 0..own {
        let finding = r.findings[i].clone();
        let Some(id) = r.details.get(i).and_then(|d| d.id.clone()) else {
            continue;
        };
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
        changed = true;
        r.repeated.retain(|t| *t != finding);
        let run = r
            .details
            .get(i)
            .and_then(|d| d.scenario.as_ref())
            .map(|s| s.run.clone())
            .or_else(|| task.findings[pos].scenarios.last().cloned())
            .filter(|run| !run.trim().is_empty());
        let Some(run) = run else {
            task.decisions.push(format!(
                "Orchestrator: review finding {id} repeated twice with no scenario; it stays open"
            ));
            continue;
        };
        let dir = app
            .store
            .run_dir(&task.id, attempt_n)
            .join(format!("finding-scenario-{id}"));
        let _ = std::fs::create_dir_all(&dir);
        let sandbox = app.task_sandbox(&task.repo, worktree, &dir);
        let results = run_verify_commands(
            CheckContext::new(app, Some(&task.id)),
            worktree,
            &dir,
            std::slice::from_ref(&run),
            sandbox.as_ref(),
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
            continue;
        }
        task.decisions.push(format!(
            "Orchestrator: review finding {id} repeated twice; its scenario passes in the worktree, so it stays open"
        ));
    }
    if changed {
        task.updated_at = now_ms();
        app.save_task_logged(task);
    }
}

/// Rules on the declines the attempt under review made, by the ids the
/// review gave them. An upheld decline resolves its open finding, and the
/// reviewer's copy of that finding is dropped. A decline that is not upheld,
/// or has no ruling, keeps the finding open and blocking: the reviewer's copy
/// stays, or the registered finding is put back into the review.
pub(super) fn apply_declines(
    task: &mut Task,
    attempt_n: u32,
    r: &mut ReviewResult,
    declines: &[Decline],
    rulings: &[DeclineRuling],
) {
    let mut ids: Vec<String> = Vec::new();
    for d in declines {
        let open = task
            .findings
            .iter()
            .any(|f| f.id == d.id && f.status == FindingStatus::Open);
        if open && !ids.contains(&d.id) {
            ids.push(d.id.clone());
        }
    }
    for id in ids {
        let Some(pos) = task.findings.iter().position(|f| f.id == id) else {
            continue;
        };
        let Some(decline) = declines.iter().find(|d| d.id == id) else {
            continue;
        };
        let text = task.findings[pos].text.clone();
        let copies: Vec<usize> = (0..r.findings.len())
            .filter(|i| r.details.get(*i).and_then(|d| d.id.as_deref()) == Some(id.as_str()))
            .collect();
        if rulings.iter().any(|g| g.id == id && g.upheld) {
            task.findings[pos].status = FindingStatus::Resolved;
            remove_findings(r, &copies);
            task.decisions.push(format!(
                "Orchestrator: review finding {id} declined by the implementer and upheld by the review; resolved: {text}"
            ));
            task.assumptions.push(Assumption {
                question: format!("Review finding declined by the implementer: {text}"),
                answer: "finding dropped".to_string(),
                evidence: decline.evidence.join("; "),
                by: "review".to_string(),
                kind: Some(QuestionKind::ReviewDispute),
                attempt: Some(attempt_n),
                commands: Vec::new(),
                overturned: false,
                owner_answer: None,
            });
            continue;
        }
        task.decisions.push(format!(
            "Orchestrator: review finding {id} declined by the implementer but not upheld; it stays open: {text}"
        ));
        if !copies.is_empty() {
            continue;
        }
        // Put the registered finding back among the reviewer's own findings.
        let at = r.severities.len().min(r.findings.len());
        let severity = brief::severity_of(&text);
        if r.details.len() < at {
            r.details.resize(at, Default::default());
        }
        r.findings.insert(at, with_id(&text, &id));
        r.severities.insert(at, severity);
        r.details.insert(
            at,
            FindingDetail {
                id: Some(id.clone()),
                ..Default::default()
            },
        );
        if matches!(severity, Some(0 | 1)) {
            r.verdict = Verdict::Fail;
        }
    }
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

    fn named(r: &mut ReviewResult, ids: &[Option<&str>]) {
        r.details = ids
            .iter()
            .map(|id| FindingDetail {
                id: id.map(str::to_string),
                ..Default::default()
            })
            .collect();
    }

    #[test]
    fn a_mark_is_added_stripped_and_read_back() {
        let marked = with_id("P1: the parser drops the last row", "F-7");
        assert_eq!(marked, "P1: the parser drops the last row [F-7]");
        assert_eq!(
            strip_finding_id(&marked),
            "P1: the parser drops the last row"
        );
        assert_eq!(
            with_id(&marked, "F-8"),
            "P1: the parser drops the last row [F-8]"
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

    #[test]
    fn a_new_p1_worded_like_a_resolved_one_with_no_id_is_kept() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut r = review(&["P1: the parser drops the last row"]);
        register_findings(&mut task, 1, 1, &mut r);
        let mut next = review(&["P2: something else"]);
        register_findings(&mut task, 2, 1, &mut next);
        assert_eq!(task.findings[0].status, FindingStatus::Resolved);

        let mut again = review(&["P1: the parser drops the last row"]);
        named(&mut again, &[None]);
        register_findings(&mut task, 3, 1, &mut again);
        assert_eq!(again.findings.len(), 1, "{:?}", again.findings);
        assert_eq!(task.findings.len(), 3);
        assert_eq!(again.details[0].id.as_deref(), Some("F-3"));
        assert!(again.repeated.is_empty());
    }

    #[test]
    fn a_named_resolved_finding_is_never_dropped_by_its_id_alone() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut r = review(&["P1: first"]);
        register_findings(&mut task, 1, 1, &mut r);
        let mut next = review(&["P1: second"]);
        register_findings(&mut task, 2, 1, &mut next);
        assert_eq!(task.findings[0].status, FindingStatus::Resolved);

        let mut back = review(&["P1: first, reworded"]);
        named(&mut back, &[Some("F-1")]);
        register_findings(&mut task, 3, 1, &mut back);
        assert_eq!(back.findings, ["P1: first, reworded [F-1]"]);
        assert_eq!(back.repeated, back.findings);
        assert_eq!(back.verdict, Verdict::Fail);
        assert_eq!(task.findings[0].status, FindingStatus::Open);
        assert_eq!(task.findings[0].repeats, HANDLE_AT_REPEATS);

        let mut next = review(&["P1: second"]);
        named(&mut next, &[Some("F-2")]);
        register_findings(&mut task, 4, 1, &mut next);
        let mut with_run = review(&["P1: first, reworded"]);
        with_run.details = vec![FindingDetail {
            id: Some("F-1".into()),
            scenario: Some(FindingScenario {
                run: "cargo test first".into(),
                fails: "red".into(),
            }),
            ..Default::default()
        }];
        register_findings(&mut task, 5, 1, &mut with_run);
        assert_eq!(task.findings[0].scenarios, ["cargo test first"]);
        assert_eq!(with_run.findings.len(), 1);
    }

    #[test]
    fn a_named_open_finding_is_a_repeat_and_counts_once_per_attempt() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut r = review(&["P1: first"]);
        register_findings(&mut task, 1, 1, &mut r);
        assert!(r.repeated.is_empty());
        for (attempt, round, repeats, repeated) in [
            (1, 1, 0, false),
            (1, 2, 0, true),
            (2, 1, 1, true),
            (2, 2, 1, true),
        ] {
            let mut again = review(&["P1: first, reworded"]);
            named(&mut again, &[Some("F-1")]);
            register_findings(&mut task, attempt, round, &mut again);
            assert_eq!(task.findings.len(), 1);
            assert_eq!(task.findings[0].repeats, repeats);
            assert_eq!(again.repeated.len(), usize::from(repeated));
        }
    }

    fn decline(id: &str) -> Decline {
        Decline {
            id: id.into(),
            reason: DeclineReason::Wrong,
            evidence: vec!["src/parse.rs:9 keeps the row".into()],
        }
    }

    fn ruling(id: &str, upheld: bool) -> DeclineRuling {
        DeclineRuling {
            id: id.into(),
            upheld,
        }
    }

    #[test]
    fn an_upheld_decline_resolves_its_finding_with_no_judge_turn() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut first = review(&["P1: the parser drops the last row"]);
        register_findings(&mut task, 1, 1, &mut first);
        let id = task.findings[0].id.clone();
        let mut next = review(&["P1: other problem entirely"]);
        apply_declines(
            &mut task,
            2,
            &mut next,
            &[decline(&id)],
            &[ruling(&id, true)],
        );
        assert_eq!(task.findings[0].status, FindingStatus::Resolved);
        assert_eq!(next.findings, ["P1: other problem entirely"]);
        assert!(task.decisions.iter().any(|d| d.contains("upheld")));
        register_findings(&mut task, 2, 1, &mut next);
        assert_eq!(task.findings[0].status, FindingStatus::Resolved);
        assert_eq!(task.findings.len(), 2);
    }

    #[test]
    fn an_upheld_decline_drops_the_reviewers_copy_of_its_finding() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut first = review(&["P1: the parser drops the last row"]);
        register_findings(&mut task, 1, 1, &mut first);
        let id = task.findings[0].id.clone();
        let mut next = review(&["P1: the parser drops the last row"]);
        named(&mut next, &[Some(id.as_str())]);
        apply_declines(
            &mut task,
            2,
            &mut next,
            &[decline(&id)],
            &[ruling(&id, true)],
        );
        assert!(next.findings.is_empty(), "{:?}", next.findings);
        assert_eq!(next.verdict, Verdict::Pass);
    }

    #[test]
    fn a_declined_finding_the_review_does_not_uphold_stays_blocking() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut first = review(&["P1: the parser drops the last row"]);
        register_findings(&mut task, 1, 1, &mut first);
        let id = task.findings[0].id.clone();
        let mut next = review(&["P1: other problem entirely"]);
        apply_declines(
            &mut task,
            2,
            &mut next,
            &[decline(&id)],
            &[ruling(&id, false)],
        );
        assert_eq!(next.verdict, Verdict::Fail);
        assert!(
            next.findings
                .iter()
                .any(|f| f == &format!("P1: the parser drops the last row [{id}]")),
            "{:?}",
            next.findings
        );
        register_findings(&mut task, 2, 1, &mut next);
        assert_eq!(task.findings[0].status, FindingStatus::Open);
        assert_eq!(next.details[1].id.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn a_decline_with_no_ruling_is_blocking_too() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut first = review(&["P1: the parser drops the last row"]);
        register_findings(&mut task, 1, 1, &mut first);
        let id = task.findings[0].id.clone();
        let mut next = review(&["P1: other problem entirely"]);
        apply_declines(&mut task, 2, &mut next, &[decline(&id)], &[]);
        assert_eq!(next.verdict, Verdict::Fail);
        register_findings(&mut task, 2, 1, &mut next);
        assert_eq!(task.findings[0].status, FindingStatus::Open);
    }

    #[test]
    fn a_p0_and_a_p3_with_the_same_wording_stay_distinct() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let mut r = review(&["P0: the same words", "P3: the same words"]);
        register_findings(&mut task, 1, 1, &mut r);
        assert_eq!(task.findings.len(), 2);
        assert_ne!(task.findings[0].id, task.findings[1].id);
    }
}
