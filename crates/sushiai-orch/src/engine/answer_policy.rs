use super::*;

/// Automatic answers per task; the next question goes to the owner.
pub(super) const MAX_POLICY_ANSWERS: usize = 3;

const JUDGE_PROMPT: &str = "A coding agent claims that an acceptance criterion cannot be met, and gives evidence. You may read files in the working directory but change nothing. Check the evidence yourself. Answer verified true only when you confirmed the evidence is true and that it really makes the criterion impossible; otherwise false. Reply with only JSON: {\"verified\": true|false, \"reason\": \"<one sentence>\"}";

const MAX_REASON_CHARS: usize = 300;

pub(super) fn policy_on(app: &App) -> bool {
    app.settings.read().unwrap().answer_policy
}

/// Questions answered so far by the policy (a rule or the judge).
fn policy_answers(task: &Task) -> usize {
    task.assumptions
        .iter()
        .filter(|a| a.by == "policy" || a.by == "judge")
        .count()
}

fn answered_before(task: &Task, kind: QuestionKind, attempt_n: Option<u32>) -> bool {
    task.assumptions.iter().any(|a| {
        (a.by == "policy" || a.by == "judge")
            && a.kind == Some(kind)
            && attempt_n.is_none_or(|n| a.attempt == Some(n))
    })
}

/// Whether the policy may answer a `kind` question of `task` at all.
pub(super) fn policy_open(app: &App, task: &Task, kind: QuestionKind) -> bool {
    policy_on(app) && !owner_only(kind) && policy_answers(task) < MAX_POLICY_ANSWERS
}

/// Kinds only the owner may allow: never the policy, the chat agent or an
/// MCP client (see [`is_refusal`] for what those may still answer).
pub(super) fn owner_only(kind: QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::Budget
            | QuestionKind::DailyBudget
            | QuestionKind::HarnessMissing
            | QuestionKind::ProtectedPath
            | QuestionKind::Permission
    )
}

/// Whether `answer` only denies or stops, so a non-owner may give it.
pub(super) fn is_refusal(question: &Question, answer: &str) -> bool {
    if answer.trim().eq_ignore_ascii_case("stop") {
        return true;
    }
    match question.kind {
        QuestionKind::Permission => {
            matches!(
                permissions::parse_choice(answer),
                permissions::Choice::Deny(_)
            )
        }
        QuestionKind::ProtectedPath => is_option(answer, "reject"),
        _ => false,
    }
}

/// Leading tokens of a repo's own CI entry. A command segment that starts
/// with one of these (after `cd <dir>` and `VAR=x`) is shared.
const CI_ENTRIES: [&[&str]; 11] = [
    &["npm", "run", "ci"],
    &["npm", "test"],
    &["npm", "run", "test"],
    &["pnpm", "test"],
    &["pnpm", "run", "ci"],
    &["pnpm", "ci"],
    &["yarn", "test"],
    &["yarn", "ci"],
    &["cargo", "test"],
    &["make", "test"],
    &["make", "ci"],
];

fn is_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether one shell segment runs a CI entry: its leading `cd <dir>` and
/// `VAR=x` words are skipped, the rest starts with an entry's tokens.
fn segment_runs_ci(segment: &str) -> bool {
    let mut tokens: Vec<&str> = segment.split_whitespace().collect();
    loop {
        match tokens.as_slice() {
            ["cd", _, ..] | ["timeout", _, ..] => tokens.drain(..2),
            ["env", ..] => tokens.drain(..1),
            [first, ..] if is_assignment(first) => tokens.drain(..1),
            _ => break,
        };
    }
    CI_ENTRIES.iter().any(|entry| tokens.starts_with(entry))
}

/// Whether any `&&`, `||`, `;` or `|` separated part of `command` runs a CI entry.
fn runs_ci_entry(command: &str) -> bool {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace([';', '|'], "\n")
        .replace(['(', ')'], " ")
        .lines()
        .any(segment_runs_ci)
}

/// Whether `command` is a check the owner or the repo stands behind: the
/// repo's CI entry, a `scopedChecks` or `afterLand` entry of the settings, or
/// a command written in the owner's own task request. Only the owner may drop
/// such a check; a planner-invented, task-specific one stays droppable.
// ponytail: a heuristic over the command text. Flag-first spellings
// (`npm run --silent ci`), `pnpm run test`, wrappers beyond cd/env/timeout and
// shell functions are missed. Upgrade path: read the repo's package.json
// scripts (and Makefile targets) and match a command against what `ci` and
// `test` really run.
pub(super) fn is_shared_check(settings: &Settings, task: &Task, command: &str) -> bool {
    let flat = |c: &str| c.split_whitespace().collect::<Vec<_>>().join(" ");
    let command = flat(command);
    if command.is_empty() {
        return false;
    }
    runs_ci_entry(&command)
        || settings.scoped_checks.iter().any(|e| {
            e.repo
                .as_deref()
                .is_none_or(|r| crate::model::same_repo_path(r, &task.repo))
                && (glob_match(&e.command, &command) || flat(&e.command) == command)
        })
        || settings
            .after_land
            .iter()
            .any(|a| crate::model::same_repo_path(&a.repo, &task.repo) && flat(&a.run) == command)
        || task
            .request
            .as_deref()
            .is_some_and(|r| flat(r).contains(&command))
}

/// The refusal for a non-owner who tries to drop `command`, a shared check.
fn shared_check_refusal(command: &str) -> String {
    format!(
        "`{command}` is a shared check (the repo's CI entry, a repo setting or the owner's own): only the owner may drop it. An agent can answer \"{}\" or \"stop\", otherwise the owner decides",
        base_check::PRE_EXISTING_RETRY
    )
}

/// Who asks for a removal: the owner, or anyone else (the overseer, a chat or
/// MCP client, the answer policy, the judge, a feasibility ruling).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Origin {
    Owner,
    Other,
}

/// The origin of the answer the wait just recorded in the question history
/// (`Owner` also when nothing was recorded).
pub(super) fn origin_of_last_answer(task: &Task) -> Origin {
    match task.question_history.last().map(|h| h.answered_by) {
        Some(AnsweredBy::Owner) | None => Origin::Owner,
        Some(_) => Origin::Other,
    }
}

/// THE choke point of check removal: every path that removes or replaces a
/// command in `verify`, `finalVerify`, `checks` or the held-out check (a drop
/// answer, a dropped criterion, an amendment, a feasibility ruling) asks here
/// first. Anyone but the owner is refused when one of `commands` is shared;
/// the caller then keeps the check and escalates to the owner.
pub(super) fn removal_refusal(
    settings: &Settings,
    task: &Task,
    commands: &[String],
    origin: Origin,
) -> Option<String> {
    if origin == Origin::Owner {
        return None;
    }
    commands
        .iter()
        .find(|c| is_shared_check(settings, task, c))
        .map(|c| shared_check_refusal(c))
}

/// The commands that go with criterion `n`: its checks, a held-out check on
/// it and the command its `-- check:` part names.
pub(super) fn criterion_commands(task: &Task, n: usize) -> Vec<String> {
    let mut out: Vec<String> = task
        .checks
        .iter()
        .chain(task.held_out.as_ref())
        .filter(|c| c.criterion == n)
        .map(|c| c.run.clone())
        .collect();
    if let Some((_, cmd)) = task.criteria.get(n).and_then(|t| t.split_once("-- check:")) {
        out.push(cmd.trim().to_string());
    }
    out
}

/// The command a criterion's `-- check:` part names.
fn text_check(text: &str) -> Option<String> {
    let (_, cmd) = text.split_once("-- check:")?;
    let cmd = cmd.trim();
    (!cmd.is_empty()).then(|| cmd.to_string())
}

/// The refusal for a judge replacing criterion `i` with `new_text` when that
/// takes a shared check out of its `-- check:` part.
pub(super) fn criterion_text_refusal(
    settings: &Settings,
    task: &Task,
    i: usize,
    new_text: &str,
) -> Option<String> {
    let old = task.criteria.get(i).and_then(|t| text_check(t))?;
    if new_text.contains(&old) {
        return None;
    }
    removal_refusal(settings, task, &[old], Origin::Other)
}

/// Removes `command` from `verify`, `finalVerify` and `checks`, unless
/// `removal_refusal` says only the owner may.
pub(super) fn remove_check_command(
    settings: &Settings,
    task: &mut Task,
    command: &str,
    origin: Origin,
) -> Result<(), String> {
    if let Some(why) = removal_refusal(settings, task, &[command.to_string()], origin) {
        return Err(why);
    }
    task.verify.retain(|c| c != command);
    task.final_verify.retain(|c| c != command);
    task.checks.retain(|c| c.run != command);
    Ok(())
}

/// Why a non-owner `answer` to `question` must not be given, when it removes
/// a shared check: "drop this check" on a base-check question, or "drop
/// criterion" on an impossible one whose criterion carries one. Only the
/// owner may; others may retry or stop. Callers that answer by rule, judge or
/// triage escalate to the owner instead.
pub(super) fn shared_check_drop_refusal(
    settings: &Settings,
    task: &Task,
    question: &Question,
    answer: &str,
) -> Option<String> {
    let commands: Vec<String> = if let Some(command) = pre_existing_command(question) {
        if !is_option(answer, base_check::PRE_EXISTING_DROP) {
            return None;
        }
        vec![command.to_string()]
    } else if let Some(n) = impossible_drop_target(question, answer) {
        criterion_commands(task, n)
    } else {
        return None;
    };
    removal_refusal(settings, task, &commands, Origin::Other)
}

/// The refusal for a non-owner amendment that removes a shared check from
/// `verify`, `finalVerify`, `checks`, the held-out check or a criterion's
/// `-- check:` part: the same rule as dropping one. `task` is the state the
/// amendment lands on (see `effective_task`).
pub(super) fn shared_check_amend_refusal(
    settings: &Settings,
    task: &Task,
    amendment: &Amendment,
) -> Option<String> {
    let gone = |old: &[String], new: &Option<Vec<String>>| -> Vec<String> {
        new.as_ref().map_or_else(Vec::new, |new| {
            old.iter().filter(|c| !new.contains(c)).cloned().collect()
        })
    };
    let old_checks: Vec<String> = task.checks.iter().map(|c| c.run.clone()).collect();
    let new_checks = amendment
        .checks
        .as_ref()
        .map(|cs| cs.iter().map(|c| c.run.clone()).collect::<Vec<_>>());
    let mut removed = [
        gone(&task.verify, &amendment.verify),
        gone(&task.final_verify, &amendment.final_verify),
        gone(&old_checks, &new_checks),
    ]
    .concat();
    // The held-out check, when the amendment replaces or clears it.
    if let (Some(old), Some(new)) = (&task.held_out, &amendment.held_out) {
        if new.as_ref().is_none_or(|n| n.run != old.run) {
            removed.push(old.run.clone());
        }
    }
    // A criterion whose `-- check:` command is no longer named by any new
    // criterion, and every criterion that falls off the end.
    if let Some(new) = &amendment.criteria {
        for (n, old) in task.criteria.iter().enumerate() {
            let check_gone = text_check(old).is_some_and(|c| !new.iter().any(|t| t.contains(&c)));
            if check_gone || n >= new.len() {
                removed.extend(criterion_commands(task, n));
            }
        }
    }
    removal_refusal(settings, task, &removed, Origin::Other)
}

/// The fixed rule for `question`: its answer and why. Each rule fires once
/// (the review rule once per attempt); a repeat goes on as before.
fn rule_answer(task: &Task, question: &Question, attempt_n: u32) -> Option<(String, String)> {
    let kind = question.kind;
    if kind == QuestionKind::PreexistingFailure
        && question
            .options
            .iter()
            .any(|o| o == base_check::PARENT_KEEP)
    {
        let kept = task.assumptions.iter().any(|a| {
            a.by == "policy" && a.kind == Some(kind) && a.answer == base_check::PARENT_KEEP
        });
        return (!kept).then(|| {
            (
                base_check::PARENT_KEEP.to_string(),
                "the subtasks are meant to make the check pass: keep it, once".to_string(),
            )
        });
    }
    let (answer, evidence, once_per) = match kind {
        QuestionKind::ReviewNoVerdict => (
            "retry",
            "a review that gave no verdict twice is reviewed again once per attempt",
            Some(attempt_n),
        ),
        QuestionKind::AttemptsFailing => (
            "continue",
            "attempts keep failing: continue once before asking the owner",
            None,
        ),
        QuestionKind::DependencyEnded => (
            "retry the dependency",
            "a dependency that ended is retried once",
            None,
        ),
        QuestionKind::PreexistingFailure => (
            base_check::PRE_EXISTING_RETRY,
            "the check already fails on the base: wait for the base to move, once",
            None,
        ),
        _ => return None,
    };
    if answered_before(task, kind, once_per) {
        return None;
    }
    Some((answer.to_string(), evidence.to_string()))
}

/// Records an automatic answer as an assumption the owner can overturn, with
/// a decision line. `by` is `policy` or `judge`.
pub(super) fn record_policy_answer(
    task: &mut Task,
    question: &Question,
    answer: &str,
    by: &str,
    evidence: &str,
    attempt_n: u32,
) {
    if question.kind == QuestionKind::AttemptsFailing {
        task.queue.rate_pauses = 0;
    }
    task.assumptions.push(Assumption {
        question: question.text.clone(),
        answer: answer.to_string(),
        evidence: evidence.to_string(),
        by: by.to_string(),
        kind: Some(question.kind),
        attempt: Some(attempt_n),
        overturned: false,
        owner_answer: None,
    });
    task.decisions
        .push(policy_answer_line(&question.text, answer, by, evidence));
    let answered_by = if by == "judge" {
        AnsweredBy::Judge
    } else {
        AnsweredBy::Policy
    };
    record_answered_question(task, question, answer, answered_by);
}

/// Keeps an answered question in the task's history: the one place every
/// answer is recorded, once per answered question.
pub(super) fn record_answered_question(
    task: &mut Task,
    question: &Question,
    answer: &str,
    answered_by: AnsweredBy,
) {
    task.question_history.push(AnsweredQuestion {
        question: question.text.clone(),
        options: question.options.clone(),
        kind: question.kind,
        asked_by: question.asked_by,
        asked_at: question.asked_at,
        answer: answer.to_string(),
        answered_at: now_ms(),
        answered_by,
    });
}

/// Answers `task`'s waiting question by rule or judge, when the policy is on
/// and allows it; `None` sends the question on (triage, then the owner). The
/// question and waiting state are cleared and persisted on an answer. Plan and
/// agent questions are left to the triage wrappers, which need triage's view.
pub(super) async fn try_policy_answer(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> Option<String> {
    let question = task.question.clone()?;
    if !policy_open(app, task, question.kind) {
        return None;
    }
    let attempt_n = task.attempts.last().map(|a| a.n).unwrap_or(0);
    let (answer, by, evidence) = match question.kind {
        QuestionKind::Impossible => {
            let (answer, reason) =
                judge_impossible(app, task, &question, attempt_n, cancel).await?;
            // Dropping a criterion that carries a shared check is the owner's.
            let refused =
                shared_check_drop_refusal(&app.settings.read().unwrap(), task, &question, &answer);
            if let Some(why) = refused {
                task.decisions.push(format!("Answer judge: {why}"));
                return None;
            }
            (answer, "judge", reason)
        }
        _ => {
            let (answer, evidence) = rule_answer(task, &question, attempt_n)?;
            (answer, "policy", evidence)
        }
    };
    record_policy_answer(task, &question, &answer, by, &evidence, attempt_n);
    app.broadcast_log(
        &task.id,
        attempt_n,
        policy_answer_line(&question.text, &answer, by, &evidence),
    );
    task.question = None;
    task.status = TaskStatus::Queued;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    Some(answer)
}

/// The criterion number and the agent's evidence of an impossible question.
fn impossible_parts(question: &Question) -> Option<(usize, String)> {
    let n = question
        .text
        .strip_prefix("Criterion ")?
        .split_once(" cannot be met as written")?
        .0
        .parse()
        .ok()?;
    let evidence = question.text.split_once(". Evidence: ")?.1.to_string();
    Some((n, evidence))
}

enum Judgement {
    Verified(String),
    Refuted(String),
}

fn parse_judgement(text: &str) -> Option<Judgement> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: serde_json::Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let reason = truncate_chars(
        &v.get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        MAX_REASON_CHARS,
    );
    match v.get("verified")?.as_bool()? {
        true => Some(Judgement::Verified(reason)),
        false => Some(Judgement::Refuted(reason)),
    }
}

/// `drop criterion` only when the judge verified the evidence, else `retry`;
/// `None` when the judge is off or gave no usable reply (the owner decides).
async fn judge_impossible(
    app: &Arc<App>,
    task: &mut Task,
    question: &Question,
    attempt_n: u32,
    cancel: &CancelToken,
) -> Option<(String, String)> {
    let (n, evidence) = impossible_parts(question)?;
    let criterion = task.criteria.get(n)?.clone();
    let (_, route, fallback) = check_route(app)?;
    let route = route?;
    if let Some(note) = fallback {
        task.decisions.push(format!("Answer judge: {note}"));
    }
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(&task.id, attempt_n).join("answer-judge");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let brief_text = format!(
        "{JUDGE_PROMPT}\n\n{}\n",
        json!({"criterion": criterion, "evidence": evidence})
    );
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let req = harness::RunRequest {
        repo_settings: false,
        ..review_request(&route, &worktree, &mcp_path, &settings_path, &[])
    };
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        RunTrack::No,
        &worktree,
        &req,
        CostTag::task("answer_judge", &route.id),
        &brief_text,
        &events_path,
        cancel,
        Guard::off(),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    // A cancelled or failed judge run asks the owner; the wait that follows
    // sees the cancellation itself.
    let outcome = result.ok()?;
    task.cost_usd += outcome.cost_usd.unwrap_or(0.0);
    if outcome.error.is_some() {
        return None;
    }
    match parse_judgement(&outcome.final_text.unwrap_or_default())? {
        Judgement::Verified(reason) => Some(("drop criterion".to_string(), reason)),
        Judgement::Refuted(reason) => Some(("retry".to_string(), reason)),
    }
}

/// The option of a plan or agent question that is phrased as the cautious or
/// reversible choice.
pub(super) fn cautious_option(options: &[String]) -> Option<&String> {
    options.iter().find(|o| {
        let o = o.to_ascii_lowercase();
        let negated = |word: &str| {
            o.contains(&format!("not {word}"))
                || o.contains(&format!("non-{word}"))
                || o.contains(&format!("in{word}"))
                || o.contains(&format!("ir{word}"))
        };
        (o.contains("cautious") && !negated("cautious"))
            || (o.contains("reversible") && !negated("reversible"))
    })
}

/// The cautious option to take for a plan or agent question if triage agrees:
/// `None` when the policy is off or spent, the kind is not one of those two, or
/// no option is marked cautious.
pub(super) fn cautious_choice(app: &App, task: &Task, question: &Question) -> Option<String> {
    if !matches!(
        question.kind,
        QuestionKind::PlanQuestion | QuestionKind::AgentQuestion
    ) || !policy_open(app, task, question.kind)
    {
        return None;
    }
    cautious_option(&question.options).cloned()
}

/// Whether triage's answer is `cautious`.
pub(super) fn agrees_with(decision: &brief::TriageDecision, cautious: Option<&str>) -> bool {
    cautious.is_some_and(|c| {
        matches!(decision.action, brief::TriageAction::Answer)
            && decision.answer.trim().eq_ignore_ascii_case(c.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_cautious_or_reversible_option_is_found() {
        let o = opts(&["rewrite it", "keep the old API (cautious)"]);
        assert_eq!(
            cautious_option(&o).map(String::as_str),
            Some("keep the old API (cautious)")
        );
        let o = opts(&["a reversible migration", "drop the table"]);
        assert_eq!(
            cautious_option(&o).map(String::as_str),
            Some("a reversible migration")
        );
    }

    #[test]
    fn a_negated_option_is_not_cautious() {
        assert!(cautious_option(&opts(&["an irreversible migration", "not cautious"])).is_none());
        assert!(cautious_option(&opts(&["yes", "no"])).is_none());
    }

    #[test]
    fn the_impossible_question_yields_its_criterion_and_evidence() {
        let q = Question::new(
            "Criterion 1 cannot be met as written: x. Evidence: the file is gone",
            vec![],
            QuestionKind::Impossible,
            AskedBy::Implement,
        );
        assert_eq!(
            impossible_parts(&q),
            Some((1, "the file is gone".to_string()))
        );
    }

    #[test]
    fn a_judgement_reads_its_flag() {
        assert!(matches!(
            parse_judgement("```json\n{\"verified\": true, \"reason\": \"seen\"}\n```"),
            Some(Judgement::Verified(r)) if r == "seen"
        ));
        assert!(matches!(
            parse_judgement("{\"verified\": false}"),
            Some(Judgement::Refuted(_))
        ));
        assert!(parse_judgement("no idea").is_none());
    }

    #[test]
    fn the_rule_never_answers_the_accept_option() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.attempts = vec![attempt_with_failure(1, "review:x")];
        let q = Question::new(
            "Attempts keep failing",
            opts(&["continue", brief::ACCEPT_LAST_ATTEMPT, "stop"]),
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        );
        let (answer, _) = rule_answer(&task, &q, 1).unwrap();
        assert_eq!(answer, "continue");
    }
}
