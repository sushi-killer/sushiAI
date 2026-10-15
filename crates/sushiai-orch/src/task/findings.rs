use crate::task::{
    Assumption, Decline, DeclineRuling, FindingDetail, FindingRecord, FindingStatus, QuestionKind,
    ReviewResult, Task, TaskData, Verdict,
};
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
fn new_finding_id(task: &TaskData) -> String {
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
pub(crate) fn move_minor_findings(task: &mut Task, attempt_n: u32, r: &mut ReviewResult) {
    let task = &mut task.data;
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
    add_follow_ups_data(task, moved);
}

/// Puts the reviewer's own findings of one review into the task's registry
/// and gives each its id: the registered finding it named, else a new one.
/// The id is in the finding's `details` and marked in its text. A finding
/// the registry holds as resolved and that comes back is kept as a repeat
/// that is handled at once: its scenario is rerun, or the next review rules on it.
/// A finding open since an earlier review run, or
/// reopened, is a repeat.
pub(crate) fn register_findings(task: &mut Task, attempt_n: u32, round: u32, r: &mut ReviewResult) {
    let task = &mut task.data;
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
                    "Orchestrator: review finding {id} came back after it was resolved; kept, its scenario or the next review rules on it"
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

pub(crate) fn apply_declines(
    task: &mut Task,
    attempt_n: u32,
    r: &mut ReviewResult,
    declines: &[Decline],
    rulings: &[DeclineRuling],
) {
    let task = &mut task.data;
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
        let severity = crate::task::review::severity_of(&text);
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

/// Adds follow-ups to the task, once each, up to a cap.
fn add_follow_ups_data(task: &mut TaskData, findings: Vec<String>) {
    for f in findings {
        let f = f.trim().chars().take(400).collect::<String>();
        if !f.is_empty()
            && !task.brief_check.follow_ups.contains(&f)
            && task.brief_check.follow_ups.len() < 12
        {
            task.brief_check.follow_ups.push(f);
        }
    }
}

impl Task {
    pub(crate) fn record_handled_finding(&mut self, observed: &FindingRecord) {
        let mut finding = observed.clone();
        finding.handled = true;
        match self
            .data
            .findings
            .iter_mut()
            .find(|record| record.id == finding.id)
        {
            Some(record) => *record = finding,
            None => self.data.findings.push(finding),
        }
    }

    pub(crate) fn add_review_follow_ups(&mut self, findings: Vec<String>) {
        add_follow_ups_data(&mut self.data, findings);
    }
}

#[cfg(test)]
#[path = "findings/tests.rs"]
mod tests;
