//! The plan gate: one typed plan review of a planner's draft, the removal of
//! the report-only criteria it names, a check that named shared checks still
//! run, one replan, and the merge of same-file subtasks into a checklist.

#[cfg(test)]
use crate::engine::planning::records::store_draft;
#[cfg(test)]
use crate::task::AttemptStatus;
#[cfg(test)]
use crate::task::Check;
#[cfg(test)]
use crate::task::Stage;

#[cfg(test)]
use crate::task::PlannedTest;

use crate::clock::now_ms;
use crate::engine::answer_policy;
use crate::engine::answer_policy::check_authority;
use crate::engine::brief_check::add_task_cost;
use crate::engine::brief_check::run_judge;
use crate::engine::brief_check::JudgeRun;
use crate::engine::planning::graph::overlapping_path;
use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::prompts::clip::truncate_chars;
use crate::prompts::engine_text;
use crate::task::checks::authority::Origin;
use crate::task::graph::has_cycle;
use crate::task::ChecklistItem;
use crate::task::PlanFinding;
use crate::task::Severity;
use crate::task::Task;
use crate::task::TaskStatus;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

const MAX_FINDING_CHARS: usize = 400;
const MAX_ITEM_CHARS: usize = 300;

/// Sections orchd appends to a subtask's request; they are not the owner's
/// own text, so no enumerated item of theirs counts.
const APPENDED_MARKERS: [&str; 2] = [
    "This is one part of a larger task",
    "Already decided for the whole task",
];

/// What the gate tells the plan stage to do next.
pub(super) enum GateAction {
    Proceed,
    /// The planner runs once more with the findings in its brief.
    Replan,
    Cancelled,
}

fn finding(kind: &str, n: usize, severity: Severity, text: String) -> PlanFinding {
    PlanFinding {
        id: format!("{kind}-{n}"),
        severity,
        kind: kind.to_string(),
        text: truncate_chars(&text, MAX_FINDING_CHARS),
    }
}

/// The owner's own words: the request up to the first section orchd added.
pub(super) fn owner_text(request: &str) -> &str {
    let end = APPENDED_MARKERS
        .iter()
        .filter_map(|m| request.find(m))
        .min()
        .unwrap_or(request.len());
    &request[..end]
}

/// A backticked span of the owner's text that is a shared check.
fn named_shared_checks(request: &str, shared: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, span) in owner_text(request).split('`').enumerate() {
        let span = span.split_whitespace().collect::<Vec<_>>().join(" ");
        if i % 2 == 1 && !span.is_empty() && shared(&span) && !out.contains(&span) {
            out.push(span);
        }
    }
    out
}

fn flat(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The shared checks the request names must still run. An exact-string check
/// is the one code check the gate keeps: a dropped shared check is critical.
fn shared_check_findings(
    request: &str,
    verify: &[String],
    final_verify: &[String],
    shared: &dyn Fn(&str) -> bool,
) -> Vec<PlanFinding> {
    let mut out = Vec::new();
    for (n, command) in named_shared_checks(request, shared).iter().enumerate() {
        let wanted = flat(command);
        let kept = verify
            .iter()
            .chain(final_verify)
            .any(|c| flat(c).contains(&wanted));
        if !kept {
            out.push(finding(
                "shared-check",
                n,
                Severity::Critical,
                format!(
                    "The request names the shared check `{command}` but neither verify nor finalVerify runs it"
                ),
            ));
        }
    }
    out
}

/// The review's kinds of finding. The schema is the contract: a reply that
/// does not match it is an unusable reply, never read leniently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ReviewKind {
    MissingRequirement,
    ReportOnly,
    Untestable,
    WrongSplit,
    WrongApproach,
}

impl ReviewKind {
    fn label(self) -> &'static str {
        match self {
            ReviewKind::MissingRequirement => "missing-requirement",
            ReviewKind::ReportOnly => "report-only",
            ReviewKind::Untestable => "untestable",
            ReviewKind::WrongSplit => "wrong-split",
            ReviewKind::WrongApproach => "wrong-approach",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReviewSeverity {
    Critical,
    High,
    Medium,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReviewFinding {
    kind: ReviewKind,
    severity: ReviewSeverity,
    text: String,
    criterion: Option<usize>,
    request_item: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewReply {
    findings: Vec<ReviewFinding>,
}

/// Reads the review's reply: the first `{` to the last `}` is the JSON, serde
/// decides the rest. A criterion index the draft does not have is unusable too.
fn parse_review(text: &str, criteria: usize) -> Result<Vec<ReviewFinding>, String> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err("the reply had no JSON".to_string());
    };
    let reply: ReviewReply = serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("the reply did not match the schema: {e}"))?;
    for (n, f) in reply.findings.iter().enumerate() {
        match f.criterion {
            Some(i) if i >= criteria => {
                return Err(format!(
                    "finding {n} names criterion {i}, but the draft has {criteria}"
                ))
            }
            None if f.kind == ReviewKind::ReportOnly => {
                return Err(format!("finding {n} is report-only but names no criterion"))
            }
            _ => {}
        }
    }
    Ok(reply.findings)
}

/// The stored form of one review finding; `n` numbers the stored ones.
fn plan_finding(n: usize, f: &ReviewFinding) -> PlanFinding {
    let body = f.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = match &f.request_item {
        Some(item) => format!("\"{}\": {body}", truncate_chars(item.trim(), 160)),
        None => body,
    };
    PlanFinding {
        id: format!("review-{n}"),
        severity: match f.severity {
            ReviewSeverity::Critical => Severity::Critical,
            ReviewSeverity::High => Severity::High,
            ReviewSeverity::Medium => Severity::Medium,
        },
        kind: f.kind.label().to_string(),
        text: truncate_chars(&text, MAX_FINDING_CHARS),
    }
}

/// The criteria a typed report-only finding names. Nothing else removes one.
fn report_only_criteria(found: &[ReviewFinding]) -> Vec<usize> {
    let mut out: Vec<usize> = found
        .iter()
        .filter(|f| f.kind == ReviewKind::ReportOnly)
        .filter_map(|f| f.criterion)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The criteria the review may remove and the findings the plan keeps. A
/// report-only criterion that carries a shared check is never removed here
/// (only the owner drops one): its finding stays, never above `High`.
fn split_review(
    settings: &Settings,
    task: &Task,
    found: &[ReviewFinding],
) -> (Vec<usize>, Vec<PlanFinding>) {
    let (removals, shared): (Vec<usize>, Vec<usize>) =
        report_only_criteria(found).into_iter().partition(|&i| {
            let commands = crate::task::checks::authority::criterion_commands(task, i);
            answer_policy::removal_refusal(
                settings,
                task,
                &commands,
                crate::task::checks::authority::Origin::Other,
            )
            .is_none()
        });
    let review = found
        .iter()
        .filter(|f| {
            f.kind != ReviewKind::ReportOnly || f.criterion.is_some_and(|i| shared.contains(&i))
        })
        .enumerate()
        .map(|(n, f)| {
            let mut finding = plan_finding(n, f);
            if f.kind == ReviewKind::ReportOnly && finding.severity == Severity::Critical {
                finding.severity = Severity::High;
            }
            finding
        })
        .collect();
    (removals, review)
}

pub(super) fn build_review_prompt(
    task: &Task,
    request: &str,
    subtasks: &[crate::engine::planning::draft::PlanSubtask],
) -> String {
    let parts: Vec<serde_json::Value> = subtasks
        .iter()
        .map(|s| {
            json!({"key": s.key, "title": s.title, "request": s.request,
                   "goal": s.goal, "criteria": s.criteria, "verify": s.verify,
                   "finalVerify": s.final_verify, "checks": s.checks,
                   "heldOutCriterion": s.held_out.as_ref().map(|h| h.criterion), "tests": s.tests,
                   "visualCriteria": s.visual_criteria, "screenshot": s.screenshot,
                   "tier": s.tier, "dependsOn": s.depends_on,
                   "paths": s.paths, "check": s.check_command()})
        })
        .collect();
    format!(
        "{}\n\n{}\n",
        engine_text("plan_gate_review"),
        json!({
            "request": request,
            "goal": task.goal,
            "criteria": task.criteria,
            "checks": task.checks,
            "heldOutCriterion": task.held_out.as_ref().map(|h| h.criterion),
            "tests": task.planner_tests.files,
            "verify": task.verify,
            "finalVerify": task.final_verify,
            "paths": task.paths,
            "subtasks": parts,
        })
    )
}

/// The `## Plan gate findings` block of a replan brief.
pub(super) fn findings_block(findings: &[PlanFinding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut out = format!("{}\n\n", engine_text("plan_gate_findings"));
    for f in findings {
        out.push_str(&format!(
            "- {} ({}): {}\n",
            f.severity.label(),
            f.kind,
            f.text
        ));
    }
    out.push('\n');
    out
}

/// The plan review's route and where it came from: the owner's `planReview`
/// when it names a configured route on an available harness, else the
/// planner's route. `Err` only when neither is usable.
fn review_route(app: &App, task: &Task) -> Result<(Route, String), String> {
    let settings = app.settings.read().unwrap().clone();
    let id = settings.plan_review.trim().to_string();
    let why = if id.is_empty() {
        "planReview is empty".to_string()
    } else {
        match settings.routes.iter().find(|r| r.id == id) {
            None => format!("route {id} is not configured"),
            Some(r) if !app.harness_available(r.harness) => {
                format!("the agent CLI of route {id} is not available")
            }
            Some(r) => return Ok((r.clone(), "planReview".to_string())),
        }
    };
    let variant = task.variant();
    let planner = variant.plan_route_id(&settings);
    match settings.routes.iter().find(|r| r.id == planner) {
        Some(r) if app.harness_available(r.harness) => {
            Ok((r.clone(), format!("the planner's route, since {why}")))
        }
        _ => Err(format!("{why}, and the planner's route is not available")),
    }
}

#[cfg(test)]
fn remove_criteria(task: &mut Task, remove: &[usize]) {
    let snapshot = crate::task::checks::snapshot::CheckSnapshot::capture(task);
    crate::task::checks::criteria::remove_report_only(
        &crate::task::checks::authority::CheckAuthority::default(),
        task,
        &snapshot,
        remove,
        Origin::Owner,
    )
    .unwrap();
}

fn is_critical(f: &PlanFinding) -> bool {
    f.severity == Severity::Critical
}

/// Runs the gate on the draft just stored on the task: the one plan review,
/// the removal of the criteria a typed report-only finding names, and the
/// shared-check check. A critical finding replans once; otherwise the
/// findings are stored for the implementer.
pub(super) async fn run(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    draft: &crate::engine::planning::draft::PlanDraft,
    cancel: &CancelToken,
) -> GateAction {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return GateAction::Proceed;
    };
    let input_snapshot = crate::task::checks::snapshot::CheckSnapshot::capture(&task);
    let settings = app.settings.read().unwrap().clone();
    let request = task.request.clone().unwrap_or_default();
    let first_pass = !task.plan_gate.reviewed;
    let carried: Vec<PlanFinding> = task
        .plan_gate
        .findings
        .iter()
        .filter(|f| f.id.starts_with("review-") && !is_critical(f))
        .cloned()
        .collect();

    // The plan review: once per task, always run.
    let mut review: Vec<PlanFinding> = Vec::new();
    let mut removals: Vec<usize> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    if first_pass {
        match review_route(app, &task) {
            Err(why) => lines.push(format!("Plan gate: plan review skipped, {why}; continuing")),
            Ok((route, source)) => {
                let prompt = build_review_prompt(&task, owner_text(&request), &draft.subtasks);
                match run_judge(
                    app,
                    &task,
                    &route,
                    Some(attempt_n),
                    "plan",
                    "plan-review",
                    &prompt,
                    cancel,
                )
                .await
                {
                    Err(RunError::Cancelled) => return GateAction::Cancelled,
                    Err(RunError::Io(msg) | RunError::NotFound(msg)) => lines.push(format!(
                        "Plan gate: plan review failed ({}); continuing",
                        truncate_chars(&msg, 200)
                    )),
                    Ok(JudgeRun {
                        reply,
                        cost,
                        timeout_note,
                    }) => {
                        add_task_cost(app, task_id, cost);
                        lines.extend(timeout_note);
                        match reply.and_then(|text| parse_review(&text, task.criteria.len())) {
                            Ok(found) => {
                                lines.push(format!(
                                    "Plan gate: plan review by {} ({source}) found {} problem(s)",
                                    route.id,
                                    found.len()
                                ));
                                (removals, review) = split_review(&settings, &task, &found);
                            }
                            Err(why) => lines.push(format!(
                                "Plan gate: plan review gave no usable answer ({}); continuing",
                                truncate_chars(&why, 200)
                            )),
                        }
                    }
                }
            }
        }
    } else {
        review = carried;
    }

    // Apply the review to exactly the check plan it read, under the task lock.
    let Ok(mut task) = app.update_task_checked(task_id, |task| {
        if !input_snapshot.matches(task) {
            return Err("the plan review is stale".to_string());
        }
        if !removals.is_empty() {
            let authority = check_authority(&settings, task);
            for index in &removals {
                if let Some(criterion) = task.criteria.get(*index) {
                    let text = criterion
                        .split_once("-- check:")
                        .map_or(criterion.as_str(), |(text, _)| text);
                    lines.push(format!(
                        "Plan gate: removed the report-only criterion \"{}\"",
                        truncate_chars(text.trim(), 160)
                    ));
                }
            }
            crate::task::checks::criteria::remove_report_only(
                &authority,
                task,
                &input_snapshot,
                &removals,
                Origin::Other,
            )?;
        }
        task.record_decisions(lines);
        if first_pass {
            task.mark_plan_reviewed();
        }
        Ok(())
    }) else {
        return GateAction::Proceed;
    };

    let replanned = task.plan_gate.replanned;
    let is_shared = |c: &str| answer_policy::is_shared_check(&settings, &task, c);
    let mut all = shared_check_findings(&request, &task.verify, &task.final_verify, &is_shared);
    all.extend(review.iter().cloned());

    if all.iter().any(is_critical) && !replanned {
        let kinds: Vec<String> = {
            let mut k: Vec<String> = all
                .iter()
                .filter(|f| is_critical(f))
                .map(|f| f.kind.clone())
                .collect();
            k.dedup();
            k
        };
        task.record_decision(format!(
            "Plan gate: {} critical finding(s) ({}); planning again once",
            all.iter().filter(|f| is_critical(f)).count(),
            kinds.join(", ")
        ));
        task.mark_replanned();
        task.record_plan_findings(all);
        // The new draft is checked again, from scratch.
        task.finish_brief_check(false);
        task.clear_missing_tool();
        task.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
        task.touch(now_ms());
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
        return GateAction::Replan;
    }

    if !all.is_empty() {
        task.record_decision(format!(
            "Plan gate: {} finding(s) left for the implementer's brief",
            all.len()
        ));
    }
    task.record_plan_findings(all);
    task.touch(now_ms());
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
    GateAction::Proceed
}

/// What the plan stage does with the planner's subtasks: parts that keep
/// their own child, and the ordered parts of the one merged task.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct MergePlan {
    pub separate: Vec<usize>,
    pub merged: Vec<usize>,
}

/// A part stays its own child when it lists paths and targeted proof.
/// Overlapping complete drafts are serialised; incomplete overlaps become
/// one merged group, in planner
/// order made stable-topological by `dependsOn`. If the group, as one node,
/// would make the dependency graph cyclic, everything merges.
pub(super) fn merge_parts(parts: &[crate::engine::planning::draft::PlanSubtask]) -> MergePlan {
    let keys: Vec<&str> = parts.iter().map(|p| p.key.trim()).collect();
    let stands_alone = |i: usize| {
        !parts[i].paths.is_empty()
            && !parts[i].check_command().is_empty()
            && !(0..parts.len()).any(|j| {
                j != i
                    && overlapping_path(&parts[i].paths, &parts[j].paths).is_some()
                    && !(parts[i].is_complete() && parts[j].is_complete())
            })
    };
    let mut separate: Vec<usize> = (0..parts.len()).filter(|&i| stands_alone(i)).collect();
    let mut merged: Vec<usize> = (0..parts.len()).filter(|i| !separate.contains(i)).collect();
    if !merged.is_empty() && !separate.is_empty() {
        const GROUP: &str = "\u{0}group";
        let node = |key: &str, group: &[usize]| -> String {
            match keys.iter().position(|k| *k == key) {
                Some(at) if group.contains(&at) => GROUP.to_string(),
                _ => key.to_string(),
            }
        };
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        for (i, part) in parts.iter().enumerate() {
            let from = if merged.contains(&i) {
                GROUP.to_string()
            } else {
                keys[i].to_string()
            };
            let entry = edges.entry(from.clone()).or_default();
            for d in &part.depends_on {
                let to = node(d.trim(), &merged);
                if to != from {
                    entry.push(to);
                }
            }
        }
        if has_cycle(&edges) {
            merged = (0..parts.len()).collect();
            separate.clear();
        }
    }
    // Stable topological order inside the group.
    let mut ordered: Vec<usize> = Vec::new();
    let mut left = merged.clone();
    while !left.is_empty() {
        let next = left
            .iter()
            .position(|&i| {
                parts[i].depends_on.iter().all(|d| {
                    keys.iter()
                        .position(|k| *k == d.trim())
                        .is_none_or(|at| !left.contains(&at) || at == i)
                })
            })
            .unwrap_or(0);
        ordered.push(left.remove(next));
    }
    MergePlan {
        separate,
        merged: ordered,
    }
}

/// The checklist of a merged group, in group order.
pub(super) fn checklist_of(
    parts: &[crate::engine::planning::draft::PlanSubtask],
    merged: &[usize],
) -> Vec<ChecklistItem> {
    merged
        .iter()
        .map(|&i| ChecklistItem {
            text: truncate_chars(parts[i].request.trim(), MAX_ITEM_CHARS),
            check: parts[i].check_command().to_string(),
            passes: false,
        })
        .collect()
}

/// Runs each checklist item's check in the worktree and records `passes`.
/// Returns the failure detail naming the failing items, or `None` when all
/// pass, there is nothing to run, or the run was cancelled.
pub(super) async fn run_checklist(
    app: &Arc<App>,
    task: &mut Task,
    worktree: &Path,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<String> {
    let runnable: Vec<usize> = (0..task.checklist.len())
        .filter(|&i| !task.checklist[i].check.trim().is_empty())
        .collect();
    if runnable.is_empty() {
        return None;
    }
    let commands: Vec<String> = runnable
        .iter()
        .map(|&i| task.checklist[i].check.clone())
        .collect();
    let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
    let results = run_verify_commands(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(app, Some(&task.id)),
            worktree,
            sandbox.as_ref(),
            &task.base_sha,
            cancel,
        ),
        &commands,
    )
    .await;
    if cancel.is_cancelled() {
        return None;
    }
    let mut failed = Vec::new();
    for (&i, result) in runnable.iter().zip(&results) {
        task.record_checklist_result(i, result.code == Some(0));
        if !task.checklist[i].passes {
            failed.push(format!(
                "Checklist item \"{}\": `{}` exited {}.\n{}",
                truncate_chars(&task.checklist[i].text, 120),
                result.command,
                result
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                result.tail
            ));
        }
    }
    task.touch(now_ms());
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    (!failed.is_empty()).then(|| failed.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(
        key: &str,
        deps: &[&str],
        paths: &[&str],
        check: &str,
    ) -> crate::engine::planning::draft::PlanSubtask {
        crate::engine::planning::draft::PlanSubtask {
            key: key.into(),
            title: key.to_uppercase(),
            request: format!("do {key}"),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            check: check.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_review_reply_must_match_the_schema() {
        let found = parse_review(
            "Here:\n```json\n{\"findings\":[{\"kind\":\"wrong-split\",\"severity\":\"critical\",\"text\":\" a  b \"},{\"kind\":\"report-only\",\"criterion\":1,\"severity\":\"high\",\"text\":\"c\"}]}\n```",
            2,
        )
        .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, ReviewKind::WrongSplit);
        assert_eq!(found[1].criterion, Some(1));
        assert!(parse_review("{\"findings\": []}", 2).unwrap().is_empty());
        for bad in [
            "no json",
            "{\"findings\": 3}",
            // A kind outside the five, a severity outside the three, no text,
            // an index the draft does not have, a report-only without one.
            "{\"findings\":[{\"kind\":\"Wrong split\",\"severity\":\"critical\",\"text\":\"x\"}]}",
            "{\"findings\":[{\"kind\":\"untestable\",\"severity\":\"low\",\"text\":\"x\"}]}",
            "{\"findings\":[{\"kind\":\"untestable\",\"severity\":\"high\"}]}",
            "{\"findings\":[{\"kind\":\"untestable\",\"criterion\":2,\"severity\":\"high\",\"text\":\"x\"}]}",
            "{\"findings\":[{\"kind\":\"report-only\",\"severity\":\"high\",\"text\":\"x\"}]}",
        ] {
            assert!(parse_review(bad, 2).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_a_typed_report_only_finding_names_a_criterion_for_removal() {
        let found = parse_review(
            "{\"findings\":[{\"kind\":\"untestable\",\"criterion\":0,\"severity\":\"high\",\"text\":\"x\"},{\"kind\":\"report-only\",\"criterion\":2,\"severity\":\"medium\",\"text\":\"y\"}]}",
            3,
        )
        .unwrap();
        assert_eq!(report_only_criteria(&found), vec![2]);
        assert!(report_only_criteria(&parse_review("{\"findings\": []}", 3).unwrap()).is_empty());
    }

    #[test]
    fn a_report_only_criterion_with_a_shared_check_is_kept_as_a_finding() {
        use crate::engine::test_support::{task_with_status, test_app};
        let (app, _dir) = test_app();
        let settings = app.settings.read().unwrap().clone();
        let mut task = task_with_status(TaskStatus::Drafting);
        task.fixture_data().criteria = vec![
            "Shared -- check: npm run ci".into(),
            "Own -- check: test -f made.txt".into(),
        ];
        task.fixture_data().request = Some("Run `npm run ci`".into());
        task.fixture_data().planner_tests.sources = vec![crate::task::SourcedCheck {
            command: "test -f made.txt".into(),
            source: crate::task::CheckSource::Planner,
        }];
        task.fixture_data().planner_tests.files = vec![crate::task::PlannedTest {
            path: "made.txt".into(),
            content: String::new(),
        }];
        let found = parse_review(
            "{\"findings\":[{\"kind\":\"report-only\",\"criterion\":0,\"severity\":\"critical\",\"text\":\"a\"},{\"kind\":\"report-only\",\"criterion\":1,\"severity\":\"medium\",\"text\":\"b\"}]}",
            2,
        )
        .unwrap();
        let (removals, review) = split_review(&settings, &task, &found);
        assert_eq!(removals, vec![1]);
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].severity, Severity::High);
        assert_eq!(review[0].kind, "report-only");
    }

    #[test]
    fn plan_gate_flags_a_dropped_shared_check_named_in_the_request() {
        let request = "Make it so, then run `npm run ci` and `echo hi`.";
        let shared = |c: &str| c == "npm run ci";
        let verify = vec!["cargo test -p x --lib".to_string()];
        let dropped = shared_check_findings(request, &verify, &[], &shared);
        assert_eq!(dropped.len(), 1, "{dropped:?}");
        assert_eq!(dropped[0].kind, "shared-check");
        assert_eq!(dropped[0].severity, Severity::Critical);
        let final_verify = vec!["npm run ci".to_string()];
        assert!(shared_check_findings(request, &[], &final_verify, &shared).is_empty());
    }

    #[test]
    fn plan_gate_moves_whole_repo_commands_to_final_verify_when_another_remains() {
        use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
        let (app, dir) = test_app();
        let mut settings = app.settings.read().unwrap().clone();
        settings.review.clear();
        let mut task = task_with_status(TaskStatus::Drafting);
        task.fixture_data().worktree = dir.path().to_string_lossy().into_owned();
        let mut attempt = attempt_with_failure(1, "drafting");
        attempt.stage = Stage::Plan;
        attempt.status = AttemptStatus::Running;
        attempt.failure = None;
        task.fixture_data().attempts.push(attempt);
        let draft = crate::engine::planning::draft::parse_plan(
            "```sushi-plan\n{\"title\":\"Run checks\",\"verify\":[\"npm run ci\",\"cargo test -p x --lib\"],\"finalVerify\":[\"npm run ci\"]}\n```",
        ).unwrap();
        store_draft(&mut task, 0, &draft, &settings);
        assert_eq!(task.verify, vec!["cargo test -p x --lib"]);
        assert_eq!(task.final_verify, vec!["npm run ci"]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("npm run ci") && d.contains("moved to finalVerify")));

        // A sole suite also runs once as the final gate, with review off.
        let sole = crate::engine::planning::draft::parse_plan(
            "```sushi-plan\n{\"title\":\"Run checks\",\"verify\":[\"npm run ci\"]}\n```",
        )
        .unwrap();
        store_draft(&mut task, 0, &sole, &settings);
        assert!(task.verify.is_empty());
        assert_eq!(task.final_verify, vec!["npm run ci"]);
    }

    #[test]
    fn the_parent_plan_review_sees_the_complete_drafts_its_children_will_run() {
        let task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
        let mut child = part("child", &[], &["child.txt"], "test -f child.txt");
        child.title = "Write child".into();
        child.goal = "Keep the requested behaviour".into();
        child.criteria = vec!["Child behaves correctly".into()];
        child.verify = vec!["test -f child.txt".into()];
        child.final_verify = vec!["cargo test -p child".into()];
        child.tests = vec![PlannedTest {
            path: "child.test.ts".into(),
            content: "assert(child())".into(),
        }];
        child.screenshot = Some("npm run screenshot:child".into());
        child.held_out = Some(Check {
            criterion: 0,
            run: "secret-hidden-check".into(),
            baseline: None,
        });
        let prompt = build_review_prompt(&task, "Build the child", &[child]);
        let json: serde_json::Value = serde_json::from_str(
            prompt
                .strip_prefix(engine_text("plan_gate_review"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        let child = &json["subtasks"][0];
        assert!(!prompt.contains("secret-hidden-check"));
        assert_eq!(child["heldOutCriterion"], 0);
        assert_eq!(child["goal"], "Keep the requested behaviour");
        assert_eq!(child["criteria"], json!(["Child behaves correctly"]));
        assert_eq!(child["verify"], json!(["test -f child.txt"]));
        assert_eq!(child["finalVerify"], json!(["cargo test -p child"]));
        assert_eq!(child["tests"][0]["content"], "assert(child())");
        assert_eq!(child["screenshot"], "npm run screenshot:child");
    }

    #[test]
    fn plan_gate_merges_overlapping_and_checkless_parts_and_keeps_the_rest() {
        let parts = vec![
            part("a", &[], &["src"], "check a"),
            part("b", &[], &["src/b.rs"], "check b"),
            part("c", &[], &["docs"], "check c"),
            part("d", &[], &[], "check d"),
            part("e", &["c"], &["web"], ""),
        ];
        let plan = merge_parts(&parts);
        assert_eq!(plan.separate, vec![2]);
        assert_eq!(plan.merged, vec![0, 1, 3, 4]);
    }

    #[test]
    fn complete_overlapping_drafts_keep_children_but_incomplete_overlaps_merge() {
        let mut a = part("a", &[], &["src/shared"], "");
        a.goal = "Add A".to_string();
        a.criteria = vec!["A is present".to_string()];
        a.verify = vec!["test -f src/shared/a".to_string()];
        let mut b = part("b", &[], &["src/shared"], "");
        b.goal = "Add B".to_string();
        b.criteria = vec!["B is present".to_string()];
        b.verify = vec!["test -f src/shared/b".to_string()];
        assert_eq!(
            merge_parts(&[a.clone(), b.clone()]),
            MergePlan {
                separate: vec![0, 1],
                merged: vec![],
            }
        );
        b.goal.clear();
        assert_eq!(
            merge_parts(&[a, b]),
            MergePlan {
                separate: vec![],
                merged: vec![0, 1],
            }
        );
    }

    #[test]
    fn plan_gate_orders_the_group_by_dependencies_and_keeps_planner_order_otherwise() {
        let parts = vec![
            part("a", &["c"], &["src"], "x"),
            part("b", &[], &["src"], "x"),
            part("c", &[], &["src"], "x"),
        ];
        assert_eq!(merge_parts(&parts).merged, vec![1, 2, 0]);
    }

    #[test]
    fn plan_gate_merges_everything_when_the_group_would_make_a_cycle() {
        // b is separate and depends on a (grouped); a depends on b through c.
        let parts = vec![
            part("a", &["b"], &[], "x"),
            part("b", &["a"], &["docs"], "x"),
        ];
        // a and b depend on each other: not a valid graph, but the group
        // mapping alone must not loop.
        let plan = merge_parts(&parts);
        assert!(plan.separate.is_empty());
        assert_eq!(plan.merged.len(), 2);

        let parts = vec![
            part("a", &[], &[], "x"),
            part("b", &["a"], &["docs"], "x"),
            part("c", &["b"], &[], "x"),
            part("d", &["b"], &["web"], "x"),
        ];
        // Group {a, c} depends on b (via c) and b depends on the group (via a).
        let plan = merge_parts(&parts);
        assert!(plan.separate.is_empty());
        assert_eq!(plan.merged, vec![0, 1, 2, 3]);
    }

    #[test]
    fn plan_gate_removes_a_criterion_and_reindexes_its_checks() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
        task.fixture_data().criteria = vec!["r".into(), "keep one".into(), "keep two".into()];
        task.fixture_data().visual_criteria = vec!["r".into(), "keep two".into()];
        task.fixture_data().checks = vec![
            Check {
                criterion: 0,
                run: "a".into(),
                baseline: None,
            },
            Check {
                criterion: 2,
                run: "c".into(),
                baseline: None,
            },
        ];
        remove_criteria(&mut task, &[0]);
        assert_eq!(task.criteria, vec!["keep one", "keep two"]);
        assert_eq!(task.visual_criteria, vec!["keep two"]);
        assert_eq!(task.checks.len(), 1);
        assert_eq!(task.checks[0].criterion, 1);
    }
}
