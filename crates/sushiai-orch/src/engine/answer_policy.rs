use super::*;

/// Automatic answers per kind of question and cause on one task; the next
/// question of that kind and cause goes on to the overseer and the owner.
pub(super) const MAX_POLICY_ANSWERS: usize = 3;

const MAX_REASON_CHARS: usize = 300;

pub(super) fn policy_on(app: &App) -> bool {
    app.settings.read().unwrap().answer_policy
}

/// Whether an earlier automatic answer was about the same check commands as
/// `question`; any other question of one kind shares its cause.
fn same_cause(earlier: &Assumption, question: &Question) -> bool {
    question.commands.is_empty() || earlier.commands == question.commands
}

/// Questions of `question`'s kind and cause answered so far by the policy (a
/// rule or the judge); an impossible question's cause is its criterion.
fn policy_answers(task: &Task, question: &Question) -> usize {
    if question.kind == QuestionKind::Impossible {
        return task
            .loop_state
            .impossible_answers
            .iter()
            .filter(|c| Some(**c) == question.criterion)
            .count();
    }
    task.assumptions
        .iter()
        .filter(|a| {
            (a.by == "policy" || a.by == "judge")
                && a.kind == Some(question.kind)
                && same_cause(a, question)
        })
        .count()
}

fn answered_before(task: &Task, kind: QuestionKind, attempt_n: Option<u32>) -> bool {
    task.assumptions.iter().any(|a| {
        (a.by == "policy" || a.by == "judge")
            && a.kind == Some(kind)
            && attempt_n.is_none_or(|n| a.attempt == Some(n))
    })
}

/// Whether the policy may answer `question` of `task` at all.
pub(super) fn policy_open(app: &App, task: &Task, question: &Question) -> bool {
    policy_on(app)
        && !owner_only(question.kind)
        && policy_answers(task, question) < MAX_POLICY_ANSWERS
}

/// Context needed by the pre-existing-failure policy. Keep filesystem and
/// store reads here so the answer rule itself remains deterministic.
fn policy_base_context(app: &App, task: &Task, question: &Question) -> (Vec<bool>, bool) {
    let changed = git::changed_files(Path::new(&task.worktree), &task.base_sha).unwrap_or_default();
    let targeted = question
        .commands
        .iter()
        .map(|command| {
            base_check::is_task_target(task, command, &changed)
                || attributed_target(app, task, command)
        })
        .collect();
    let may_land_on_default = app.settings.read().unwrap().may_land_on_default(&task.repo);
    let base_moves = task.base_ref.as_ref().is_some_and(|base_ref| {
        // Nothing lands on a default branch that landing may not move, so a
        // wait for it to move would never end.
        if !may_land_on_default
            && git::default_branch(Path::new(&task.repo)).as_deref() == Some(base_ref.as_str())
        {
            return false;
        }
        app.store
            .list_tasks()
            .unwrap_or_default()
            .iter()
            .any(|other| {
                other.id != task.id
                    && !other.archived
                    // Only a task that can really land moves the base; one
                    // that waits (for an answer, or for this very move)
                    // moves nothing, so two red tasks never wait on each other.
                    && matches!(other.status, TaskStatus::Running | TaskStatus::Landing)
                    && !waits_for_base_move(other)
                    && crate::model::same_repo_path(&other.repo, &task.repo)
                    && other.base_ref.as_ref() == Some(base_ref)
            })
    });
    (targeted, base_moves)
}

/// Kinds only the owner may allow: never the policy, the chat agent or an
/// MCP client (see [`is_refusal`] for what those may still answer).
pub(crate) fn owner_only(kind: QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::Budget
            | QuestionKind::DailyBudget
            | QuestionKind::HarnessMissing
            | QuestionKind::ProtectedPath
            | QuestionKind::Permission
    )
}

/// Whether `answer` only asks the waiting loop to look again at a harness or
/// a daily budget that may have cleared; it allows nothing, so a non-owner
/// may give it.
pub(super) fn is_recheck(question: &Question, answer: &str) -> bool {
    matches!(
        question.kind,
        QuestionKind::HarnessMissing | QuestionKind::DailyBudget
    ) && answer.trim().eq_ignore_ascii_case("retry")
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
/// with one of these (after `cd <dir>` and `VAR=x`) is a CI entry.
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

pub(super) fn flat_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn is_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether one shell segment runs a CI entry.
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

/// The `&&`, `||`, `;`, `|` and newline separated parts of `command`.
fn command_segments(command: &str) -> Vec<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace([';', '|'], "\n")
        .replace(['(', ')'], " ")
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether any part of `command` runs a CI entry.
pub(super) fn runs_ci_entry(command: &str) -> bool {
    command_segments(command)
        .iter()
        .any(|segment| segment_runs_ci(segment))
}

/// Whether `command` is named in the owner's request.
fn named_in_request(request: Option<&str>, command: &str) -> bool {
    let command = flat_command(command);
    !command.is_empty() && request.is_some_and(|r| flat_command(r).contains(&command))
}

/// A plain file name: no glob, and an extension after a non-empty name.
fn is_plain_file(target: &str) -> bool {
    !target.contains(['*', '?', '['])
        && target
            .rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(name, ext)| {
                !name.is_empty()
                    && !name.ends_with('.')
                    && ext.chars().any(|c| c.is_ascii_alphabetic())
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
}

/// Whether every part of `command` names only targets the planner made: a
/// planner test file, or a plain file that is absent on the base. A bare
/// word, `$(…)` or a backtick in it, or a part without a target (`go test
/// ./...`), fails.
fn made_by_planner(task: &Task, command: &str) -> bool {
    let command = flat_command(command);
    let absent = base_check::targets_absent_on_base(&task.repo, &task.base_sha, &command);
    let planner_made = |target: &String| {
        let target = target.strip_prefix("./").unwrap_or(target);
        task.planner_tests.files.iter().any(|f| f.path == target)
            || (is_plain_file(target) && absent.iter().any(|a| a == target))
    };
    let segments = command_segments(&command);
    !segments.is_empty()
        && segments.iter().all(|segment| {
            if segment.contains("$(") || segment.contains('`') {
                return false;
            }
            let targets = base_check::check_targets(segment);
            // Every word after the runner must be a flag or one of the
            // targets: a bare word may name a directory or a path of the base.
            let words = segment
                .split_whitespace()
                .skip_while(|word| is_assignment(word))
                .skip(1)
                .filter(|word| !word.starts_with('-'))
                .count();
            !targets.is_empty() && words == targets.len() && targets.iter().all(planner_made)
        })
}

/// The source of one command of a plan, ruled once when the plan is stored:
/// the owner's when the request names it, the repo's when it is a
/// whole-repo suite or a verify command that is not about files the planner
/// made, else the planner's.
pub(super) fn classify_check_source(
    task: &Task,
    cwd: &Path,
    command: &str,
    in_verify: bool,
) -> CheckSource {
    if named_in_request(task.request.as_deref(), command) {
        CheckSource::Owner
    } else if verify::whole_repo_suite(cwd, command)
        || (in_verify && !made_by_planner(task, command))
    {
        CheckSource::Repo
    } else {
        CheckSource::Planner
    }
}

/// Older tasks stored the commands of the plan as bare strings. The same
/// ruling as at plan time gives each its source, with the repo directory in
/// place of the worktree; what it cannot decide is the repo's.
pub(crate) fn migrate_check_sources(task: &mut Task) {
    if task.planner_tests.legacy_commands.is_empty() {
        return;
    }
    let legacy = std::mem::take(&mut task.planner_tests.legacy_commands);
    let repo = PathBuf::from(&task.repo);
    task.record_check_sources(|t, c, in_verify| {
        if legacy.iter().any(|l| l == c) {
            classify_check_source(t, &repo, c, in_verify)
        } else {
            CheckSource::Repo
        }
    });
    // Only the commands the old build recorded are the plan's.
    task.planner_tests
        .sources
        .retain(|s| legacy.contains(&s.command));
}

/// Whether only the owner may drop `command`. An agent may drop a check only
/// when its recorded source is the planner, no setting names it and it runs
/// no CI entry. A check
/// of the repo or of the owner, and a command nobody recorded, is shared.
pub(super) fn is_shared_check(settings: &Settings, task: &Task, command: &str) -> bool {
    let command = flat_command(command);
    if command.is_empty() {
        return false;
    }
    let named_by_setting = scoped_checks_for(settings, task).iter().any(|e| {
        e.repo
            .as_deref()
            .is_none_or(|r| crate::model::same_repo_path(r, &task.repo))
            && (glob_match(&e.command, &command) || flat_command(&e.command) == command)
    }) || settings.after_land.iter().any(|a| {
        crate::model::same_repo_path(&a.repo, &task.repo) && flat_command(&a.run) == command
    });
    let source = task
        .planner_tests
        .sources
        .iter()
        .find(|s| flat_command(&s.command) == command)
        .map_or(CheckSource::Repo, |s| s.source);
    named_by_setting || source != CheckSource::Planner || runs_ci_entry(&command)
}

/// The refusal for a non-owner who tries to drop `command`, a shared check.
fn shared_check_refusal(command: &str) -> String {
    format!(
        "`{command}` is a shared check (not made by the planner for this task, or named by a setting or the owner): only the owner may drop it. An agent can answer \"{}\" or \"stop\", otherwise the owner decides",
        PRE_EXISTING_RETRY
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
/// a shared check: "drop all" on a base-check question, or "drop
/// criterion" on an impossible one whose criterion carries one. Only the
/// owner may; others may retry or stop. Callers that answer by rule, judge or
/// the overseer escalates to the owner instead.
pub(super) fn shared_check_drop_refusal(
    settings: &Settings,
    task: &Task,
    question: &Question,
    answer: &str,
) -> Option<String> {
    let commands: Vec<String> = if question.kind == QuestionKind::PreexistingFailure {
        if !is_option(answer, base_check::PRE_EXISTING_DROP) {
            return None;
        }
        question.commands.clone()
    } else {
        criterion_commands(task, impossible_drop_target(question, answer)?)
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
    // A check run that a rebuild from a new `-- check:` replaces.
    for (i, command) in amendment.rebuilds(task) {
        removed.extend(
            task.checks
                .iter()
                .filter(|c| c.criterion == i && c.run != command)
                .map(|c| c.run.clone()),
        );
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

/// Whether the policy already gave `answer` to every check `question` is
/// about: a base-check rule fires once per command, not once per task. A
/// question without commands counts as one unnamed command.
fn covered_by_rule(task: &Task, question: &Question, answer: &str) -> bool {
    let earlier = || {
        task.assumptions.iter().filter(|a| {
            (a.by == "policy" || a.by == "judge")
                && a.kind == Some(question.kind)
                && a.answer == answer
        })
    };
    if question.commands.is_empty() {
        return earlier().next().is_some();
    }
    question.commands.iter().all(|c| {
        earlier().any(|a| a.commands.contains(c))
            || (answer == PRE_EXISTING_RETRY && base_check::non_owner_base_retry(task, c, None))
    })
}

/// The fixed rule for `question`: its answer and why. Each rule fires once
/// (the review rule once per attempt); a repeat goes on as before.
fn rule_answer_with_context(
    task: &Task,
    question: &Question,
    attempt_n: u32,
    targeted: &[bool],
    base_moves: bool,
) -> Option<(String, String)> {
    let kind = question.kind;
    let (answer, evidence, once_per) = match kind {
        QuestionKind::ReviewNoVerdict => (
            "retry",
            "a review that gave no verdict twice is reviewed again once per attempt",
            Some(attempt_n),
        ),
        QuestionKind::DependencyEnded => (
            "retry the dependency",
            "a dependency that ended is retried once",
            None,
        ),
        QuestionKind::PreexistingFailure => {
            if question
                .options
                .iter()
                .any(|option| option == base_check::KEEP_ALL)
            {
                (
                    base_check::KEEP_ALL,
                    "the subtasks are meant to make the check pass: keep it, once",
                    None,
                )
            } else {
                let all_targeted = !targeted.is_empty() && targeted.iter().all(|value| *value);
                if all_targeted {
                    (
                        base_check::KEEP_ALL,
                        "the task is meant to make the check pass",
                        None,
                    )
                } else if base_moves {
                    (
                        PRE_EXISTING_RETRY,
                        "another live task targets this base branch: wait for it to move, once",
                        None,
                    )
                } else {
                    return None;
                }
            }
        }
        _ => return None,
    };
    // A review that may not run again offers no `retry`: nothing to answer.
    if kind == QuestionKind::ReviewNoVerdict && !question.options.iter().any(|o| o == answer) {
        return None;
    }
    let answered = if kind == QuestionKind::PreexistingFailure {
        covered_by_rule(task, question, answer)
    } else {
        answered_before(task, kind, once_per)
    };
    if answered {
        return None;
    }
    Some((answer.to_string(), evidence.to_string()))
}

#[cfg(test)]
fn rule_answer(task: &Task, question: &Question, attempt_n: u32) -> Option<(String, String)> {
    rule_answer_with_context(task, question, attempt_n, &[false], true)
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
        commands: question.commands.clone(),
        overturned: false,
        owner_answer: None,
    });
    if question.kind == QuestionKind::Impossible {
        task.loop_state
            .impossible_answers
            .extend(question.criterion);
    }
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
    if question.kind == QuestionKind::PreexistingFailure {
        let base_sha = task.base_sha.clone();
        let by = base_check::answerer_name(Some(answered_by));
        base_check::record_base_answer(task, &question.commands, &base_sha, by, answer);
    }
    // The owner's answers do not count against the orchestrator's continues.
    if question.kind == QuestionKind::AttemptsFailing
        && matches!(
            answered_by,
            AnsweredBy::Orchestrator | AnsweredBy::Overseer | AnsweredBy::Lead
        )
        && !is_option(answer, "stop")
    {
        task.loop_state.non_owner_continues += 1;
    }
    task.question_history.push(AnsweredQuestion {
        question: question.text.clone(),
        options: question.options.clone(),
        kind: question.kind,
        asked_by: question.asked_by,
        asked_at: question.asked_at,
        answer: answer.to_string(),
        answered_at: now_ms(),
        answered_by,
        failure_signature: question.failure_signature.clone(),
        commands: question.commands.clone(),
    });
}

/// Answers `task`'s waiting question by rule or judge, when the policy is on
/// and allows it; `None` sends the question on (the overseer, then the owner).
/// The question and waiting state are cleared and persisted on an answer. Plan
/// and agent questions have no rule: the overseer decides them.
pub(super) async fn try_policy_answer(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> Option<String> {
    if cancel.is_cancelled() {
        return None;
    }
    let question = task.question.clone()?;
    if !policy_open(app, task, &question) {
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
            let (targeted, base_moves) = if question.kind == QuestionKind::PreexistingFailure {
                policy_base_context(app, task, &question)
            } else {
                (Vec::new(), false)
            };
            let (answer, evidence) =
                rule_answer_with_context(task, &question, attempt_n, &targeted, base_moves)?;
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
    app.save_task_logged(task);
    app.broadcast_task(task);
    Some(answer)
}

/// The criterion number and the agent's evidence of an impossible question.
fn impossible_parts(question: &Question) -> Option<(usize, String)> {
    Some((question.criterion?, question.evidence.clone()?))
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

pub(super) fn impossible_brief(criterion: &str, evidence: &str) -> String {
    format!(
        "{}\n\n{}\n",
        engine_text("answer_policy_judge"),
        json!({"criterion": criterion, "evidence": evidence})
    )
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
    let (_, route, fallback) = check_route_for(app, &task.variant())?;
    let route = route?;
    if let Some(note) = fallback {
        task.decisions.push(format!("Answer judge: {note}"));
    }
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "answer-judge", Some(attempt_n))
        .ok()?;
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
    let brief_text = impossible_brief(&criterion, &evidence);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let req = harness::RunRequest {
        repo_settings: false,
        resume: None,
        ..review_request(&route, &worktree, &mcp_path, &settings_path, &[])
    };
    let limit = app.helper_timeout();
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
        Guard::helper("answer judge", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    // A cancelled or failed judge run asks the owner; the wait that follows
    // sees the cancellation itself.
    let outcome = result.ok()?;
    if outcome.timed_out {
        task.decisions
            .push(helper_timeout_message("answer judge", limit));
    }
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

/// The option taken for a waiting question nobody answers in time: the
/// `default_option` the asking agent named, else the option marked cautious on
/// a plan or agent question. `None` when the policy is off or spent, or the
/// question names neither.
pub(super) fn default_choice(app: &App, task: &Task, question: &Question) -> Option<String> {
    if !policy_open(app, task, question) {
        return None;
    }
    let under_limit = |option: &String| hard_limit(question, option).is_some();
    if let Some(index) = question.default_option {
        return question
            .options
            .get(index)
            .filter(|o| !under_limit(o))
            .cloned();
    }
    if !matches!(
        question.kind,
        QuestionKind::PlanQuestion | QuestionKind::AgentQuestion
    ) {
        return None;
    }
    cautious_option(&question.options)
        .filter(|o| !under_limit(o))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_wait_default_never_takes_an_option_under_a_hard_limit() {
        let (app, _dir) = crate::engine::test_support::test_app();
        app.settings.write().unwrap().answer_policy = true;
        let task = task_with_status(TaskStatus::Waiting);
        let mut question = Question::new(
            "Which layout?",
            opts(&["wide", "narrow"]),
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        );
        question.default_option = Some(1);
        assert_eq!(
            default_choice(&app, &task, &question).as_deref(),
            Some("narrow")
        );
        question.text = "Clean up the old data?".into();
        question.options = opts(&["delete the old data", "keep the old data"]);
        assert_eq!(default_choice(&app, &task, &question), None);
        question.default_option = Some(0);
        assert_eq!(default_choice(&app, &task, &question), None);
        // The agent's own tag limits an option the words do not.
        question.options = opts(&["go ahead", "wait"]);
        question.text = "How should it go?".into();
        question.risk = vec![Risk::Destructive];
        assert_eq!(default_choice(&app, &task, &question), None);
    }

    #[test]
    fn a_check_is_the_owners_unless_the_planner_wrote_it_for_a_file_of_its_own() {
        let settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        planner_made(&mut task, "pytest tests/test_new.py", "tests/test_new.py");
        planner_made(
            &mut task,
            "node --test tests/a.test.cjs README.md",
            "tests/a.test.cjs",
        );
        for command in [
            "go test ./...",
            "pytest",
            "cargo nextest run",
            "just ci",
            "./gradlew test",
            "npx vitest run",
            "npm run test:daemon",
            "pytest tests/test_other.py",
            "",
        ] {
            assert_eq!(
                is_shared_check(&settings, &task, command),
                !command.is_empty(),
                "{command}"
            );
        }
        assert!(!is_shared_check(
            &settings,
            &task,
            "pytest   tests/test_new.py"
        ));
    }

    #[test]
    fn a_planner_command_that_the_owner_or_the_settings_name_stays_the_owners() {
        let mut settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        planner_made(&mut task, "pytest tests/test_new.py", "tests/test_new.py");
        assert!(!is_shared_check(
            &settings,
            &task,
            "pytest tests/test_new.py"
        ));
        task.planner_tests.sources[0].source = CheckSource::Owner;
        assert!(is_shared_check(
            &settings,
            &task,
            "pytest tests/test_new.py"
        ));
        task.planner_tests.sources[0].source = CheckSource::Planner;
        settings.after_land.push(AfterLand {
            repo: task.repo.clone(),
            run: "pytest tests/test_new.py".into(),
        });
        assert!(is_shared_check(
            &settings,
            &task,
            "pytest tests/test_new.py"
        ));
    }

    fn sourced(task: &mut Task, command: &str, source: CheckSource) {
        task.planner_tests.sources.push(SourcedCheck {
            command: command.into(),
            source,
        });
    }

    #[test]
    fn a_whole_suite_in_verify_is_the_repos_and_a_planner_file_check_is_not() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = repo.path().to_string_lossy().into_owned();
        task.base_sha = git_in(repo.path(), &["rev-parse", "HEAD"]);
        let suites = [
            "go test ./...",
            "pytest",
            "just ci",
            "./gradlew test",
            "tox",
            "make check",
            "cargo nextest run",
            "cargo test --workspace",
            "pytest tests tests/new_test.py",
        ];
        task.verify = suites.iter().map(|c| c.to_string()).collect();
        task.verify.push("test -f new/a.txt".into());
        task.criteria = vec!["x -- check: go test ./...".into()];
        task.record_check_sources(|t, c, v| classify_check_source(t, repo.path(), c, v));
        let settings = Settings::default();
        for command in suites {
            assert!(is_shared_check(&settings, &task, command), "{command}");
        }
        assert!(!is_shared_check(&settings, &task, "test -f new/a.txt"));
    }

    #[test]
    fn a_planner_check_that_cannot_pass_on_a_moved_base_is_droppable() {
        let settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.criteria = vec![
            "The version is 9 -- check: grep -qF 'pub const V: u32 = 9;' src/brief.rs && git diff --quiet".into(),
        ];
        let command = "grep -qF 'pub const V: u32 = 9;' src/brief.rs && git diff --quiet";
        sourced(&mut task, command, CheckSource::Planner);
        assert!(!is_shared_check(&settings, &task, command));
        assert_eq!(
            removal_refusal(&settings, &task, &[command.into()], Origin::Other),
            None
        );
    }

    #[test]
    fn a_repo_or_owner_check_stays_shared_for_a_non_owner() {
        let mut settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        sourced(&mut task, "npm test", CheckSource::Repo);
        sourced(&mut task, "cargo test --workspace", CheckSource::Repo);
        sourced(&mut task, "grep -q x a.txt", CheckSource::Owner);
        for command in [
            "npm test",
            "cargo test --workspace",
            "grep -q x a.txt",
            "make unknown",
        ] {
            assert!(is_shared_check(&settings, &task, command), "{command}");
            assert!(
                removal_refusal(&settings, &task, &[command.into()], Origin::Other).is_some(),
                "{command}"
            );
            assert_eq!(
                removal_refusal(&settings, &task, &[command.into()], Origin::Owner),
                None
            );
        }
        // A setting that names a planner-recorded command keeps it shared.
        sourced(&mut task, "pytest tests/x.py", CheckSource::Planner);
        assert!(!is_shared_check(&settings, &task, "pytest tests/x.py"));
        settings.after_land.push(AfterLand {
            repo: task.repo.clone(),
            run: "pytest tests/x.py".into(),
        });
        assert!(is_shared_check(&settings, &task, "pytest tests/x.py"));
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
        let mut task = task_with_status(TaskStatus::Running);
        task.criteria = vec!["zero".into(), "one. Evidence: nope".into()];
        // The criterion and the evidence hold the old separator: nothing is parsed back.
        let evidence = "see a. Evidence: b";
        let claim = brief::Impossible {
            criterion: 1,
            evidence: evidence.into(),
        };
        let q = impossible_question(&task, &claim);
        assert_eq!(impossible_parts(&q), Some((1, evidence.to_string())));
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
    fn attempts_keep_failing_has_no_blind_continue_rule() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.attempts = vec![attempt_with_failure(1, "review:x")];
        let q = Question::new(
            "Attempts keep failing",
            opts(&["continue", "stop"]),
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        );
        // The failure ladder owns this kind: a changed approach, never a
        // plain continue.
        assert!(rule_answer(&task, &q, 1).is_none());
    }

    fn base_check_question(commands: &[&str], options: &[&str]) -> Question {
        let failed: Vec<VerifyOutcome> = commands
            .iter()
            .map(|c| VerifyOutcome {
                command: c.to_string(),
                code: Some(1),
                tail: "boom".to_string(),
                ms: 1,
                ..Default::default()
            })
            .collect();
        let mut q = base_check::pre_existing_question(&failed, "0123456789");
        q.options = opts(options);
        q
    }

    #[test]
    fn drop_all_with_a_shared_check_is_refused_for_a_non_owner() {
        let settings = Settings::default();
        let task = task_with_status(TaskStatus::Waiting);
        let q = base_check_question(
            &["node --test tests/x.test.cjs", "npm run ci"],
            &["retry after the base is fixed", "drop all", "stop"],
        );
        let mut task = task;
        planner_made(
            &mut task,
            "node --test tests/x.test.cjs",
            "tests/x.test.cjs",
        );
        let why = shared_check_drop_refusal(&settings, &task, &q, "drop all").unwrap();
        assert!(
            why.contains("`npm run ci`") && why.contains("only the owner"),
            "{why}"
        );
        // Retry and stop are not removals.
        assert!(shared_check_drop_refusal(&settings, &task, &q, "stop").is_none());
        // Only task-specific checks: droppable by anyone.
        let q = base_check_question(&["node --test tests/x.test.cjs"], &["drop all"]);
        assert!(shared_check_drop_refusal(&settings, &task, &q, "drop all").is_none());
        // Nothing is removed on a refusal: the owner's route is the only one.
        task.verify = vec!["npm run ci".into(), "node --test tests/x.test.cjs".into()];
        let q = base_check_question(
            &["npm run ci", "node --test tests/x.test.cjs"],
            &["drop all"],
        );
        assert!(removal_refusal(&settings, &task, &q.commands, Origin::Other).is_some());
        assert_eq!(task.verify.len(), 2);
    }

    #[test]
    fn a_base_check_rule_fires_once_per_command() {
        let mut task = task_with_status(TaskStatus::Waiting);
        let keep = base_check::KEEP_ALL;
        let first = base_check_question(&["a.sh", "b.sh"], &[keep, "drop all", "stop"]);
        let (answer, _) = rule_answer_with_context(&task, &first, 1, &[true, true], false).unwrap();
        assert_eq!(answer, keep);
        record_policy_answer(&mut task, &first, &answer, "policy", "e", 1);
        // The same commands again: covered.
        assert!(rule_answer_with_context(&task, &first, 1, &[true, true], false).is_none());
        // A new command in the list: the rule fires again.
        let second = base_check_question(&["b.sh", "c.sh"], &[keep, "drop all", "stop"]);
        assert!(rule_answer_with_context(&task, &second, 1, &[true, true], false).is_some());
        record_policy_answer(&mut task, &second, keep, "policy", "e", 1);
        assert!(rule_answer_with_context(&task, &second, 1, &[true, true], false).is_none());

        // The retry rule of a single task is per command too.
        let retry = PRE_EXISTING_RETRY;
        let mut task = task_with_status(TaskStatus::Waiting);
        let one = base_check_question(&["a.sh"], &[retry, "drop all", "stop"]);
        let (answer, _) = rule_answer_with_context(&task, &one, 1, &[false], true).unwrap();
        assert_eq!(answer, retry);
        record_policy_answer(&mut task, &one, &answer, "policy", "e", 1);
        assert!(rule_answer_with_context(&task, &one, 1, &[false], true).is_none());
        let other = base_check_question(&["a.sh", "d.sh"], &[retry, "drop all", "stop"]);
        assert!(rule_answer_with_context(&task, &other, 1, &[false, false], true).is_some());
    }

    #[test]
    fn preexisting_policy_keeps_targets_retries_only_moving_bases_and_otherwise_defers() {
        let mut task = task_with_status(TaskStatus::Waiting);
        let command = "node --test tests/focus.test.cjs";
        task.request = Some(format!("Make {command} pass"));
        let question = base_check::pre_existing_question(
            &[VerifyOutcome {
                command: command.into(),
                code: Some(1),
                tail: "test a_base_retry_carries_the_planner_test_commit ... FAILED".into(),
                ..Default::default()
            }],
            &task.base_sha,
        );
        let targeted = base_check::is_task_target(&task, command, &[]);
        let (answer, evidence) =
            rule_answer_with_context(&task, &question, 1, &[targeted], false).unwrap();
        assert_eq!(answer, base_check::KEEP_ALL);
        assert!(evidence.contains("task is meant to make the check pass"));

        task.request = None;
        let path_command = "node --test crates/sushiai-orch/tests/base_retry.rs";
        let path_output = "error in crates/sushiai-orch/tests/base_retry.rs:17:2";
        let path_target = base_check::is_task_target(
            &task,
            path_command,
            &["crates/sushiai-orch/tests/base_retry.rs".into()],
        );
        assert!(path_target);
        let path_question = base_check::pre_existing_question(
            &[VerifyOutcome {
                command: path_command.into(),
                code: Some(1),
                tail: path_output.into(),
                ..Default::default()
            }],
            &task.base_sha,
        );
        let (answer, _) =
            rule_answer_with_context(&task, &path_question, 1, &[path_target], false).unwrap();
        assert_eq!(answer, base_check::KEEP_ALL);

        let control =
            base_check_question(&["make verify"], &[PRE_EXISTING_RETRY, "drop all", "stop"]);
        assert!(rule_answer_with_context(&task, &control, 1, &[false], false).is_none());
        let (answer, _) = rule_answer_with_context(&task, &control, 1, &[false], true).unwrap();
        assert_eq!(answer, PRE_EXISTING_RETRY);

        let parent_options = base_check_question(
            &["make verify"],
            &[base_check::KEEP_ALL, "drop all", "stop"],
        );
        for base_moves in [false, true] {
            let (answer, evidence) =
                rule_answer_with_context(&task, &parent_options, 1, &[false], base_moves).unwrap();
            assert_eq!(answer, base_check::KEEP_ALL);
            assert!(evidence.contains("subtasks are meant to make the check pass"));
        }

        let mixed = base_check_question(
            &[
                "cargo test -p sushiai-orch --test base_retry",
                "make verify",
            ],
            &[PRE_EXISTING_RETRY, "drop all", "stop"],
        );
        let (answer, _) = rule_answer_with_context(&task, &mixed, 1, &[true, false], true).unwrap();
        assert_eq!(answer, PRE_EXISTING_RETRY);
        assert!(rule_answer_with_context(&task, &mixed, 1, &[true, false], false).is_none());
    }
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    #[test]
    fn a_command_retried_by_a_lead_overseer_or_chat_is_not_retried_by_rule() {
        for caller in [
            AnsweredBy::Lead,
            AnsweredBy::Overseer,
            AnsweredBy::Orchestrator,
            AnsweredBy::Owner,
        ] {
            let mut task = task_with_status(TaskStatus::Waiting);
            let first = base_check::pre_existing_question(
                &[VerifyOutcome {
                    command: "a.sh".into(),
                    code: Some(1),
                    ..Default::default()
                }],
                &task.base_sha,
            );
            assert!(rule_answer(&task, &first, 1).is_some());
            record_answer(&mut task, Some(&first), PRE_EXISTING_RETRY, caller);
            let sha = task.base_sha.clone();
            record_owner_base_check(&mut task, &first.commands, &sha, PRE_EXISTING_RETRY, false);
            assert_eq!(
                rule_answer(&task, &first, 1).is_none(),
                caller != AnsweredBy::Owner
            );
            let mut new_command = first.clone();
            new_command.commands.push("b.sh".into());
            assert!(rule_answer(&task, &new_command, 1).is_some());
            task.base_sha = "1234567890".into();
            assert_eq!(
                rule_answer(&task, &first, 1).is_none(),
                caller != AnsweredBy::Owner
            );
        }
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn answer_judge_timed_out_leaves_the_decision_to_the_owner() {
        let (app, _dir, mut task, _) = slow_helper_task();
        let criteria = task.criteria.clone();
        let claim = brief::Impossible {
            criterion: 0,
            evidence: "the file is gone".into(),
        };
        let question = impossible_question(&task, &claim);
        assert!(
            judge_impossible(&app, &mut task, &question, 1, &CancelToken::new())
                .await
                .is_none()
        );
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "answer judge");
    }
}

#[cfg(test)]
mod base_moves_tests {
    use super::*;
    use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};

    #[test]
    fn a_default_branch_landing_may_not_move_is_never_waited_for() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);
        git_in(&repo, &["branch", "-M", "main"]);
        let on_main = |status| {
            let mut task = task_with_status(status);
            task.repo = repo.display().to_string();
            task.worktree = task.repo.clone();
            task.base_ref = Some("main".into());
            task.base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
            task
        };
        let task = on_main(TaskStatus::Waiting);
        let mut other = on_main(TaskStatus::Running);
        other.id = "other".into();
        app.store.save_task(&task).unwrap();
        app.store.save_task(&other).unwrap();
        let question = Question::new(
            "make verify already fails on base",
            vec![PRE_EXISTING_RETRY.into(), "stop".into()],
            QuestionKind::PreexistingFailure,
            AskedBy::Verify,
        );

        let (_, base_moves) = policy_base_context(&app, &task, &question);
        assert!(!base_moves, "nothing may land on main, so it never moves");

        app.settings
            .write()
            .unwrap()
            .land_on_default_repos
            .insert(task.repo.clone(), true);
        let (_, base_moves) = policy_base_context(&app, &task, &question);
        assert!(base_moves, "another live task may land on main");
    }

    #[test]
    fn only_a_task_that_can_land_counts_as_moving_the_base() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);
        git_in(&repo, &["branch", "-M", "main"]);
        app.settings
            .write()
            .unwrap()
            .land_on_default_repos
            .insert(repo.display().to_string(), true);
        let on_main = |id: &str, status| {
            let mut task = task_with_status(status);
            task.id = id.into();
            task.repo = repo.display().to_string();
            task.worktree = task.repo.clone();
            task.base_ref = Some("main".into());
            task.base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
            task
        };
        let red = on_main("red", TaskStatus::Waiting);
        app.store.save_task(&red).unwrap();
        let question = Question::new(
            "make verify already fails on base",
            vec![PRE_EXISTING_RETRY.into(), "stop".into()],
            QuestionKind::PreexistingFailure,
            AskedBy::Verify,
        );
        let moves = |app: &App| policy_base_context(app, &red, &question).1;

        // Another red task, waiting for an answer, queued, or in the bounded
        // wait for this very move, can land nothing.
        let mut asking = on_main("asking", TaskStatus::Waiting);
        let queued = on_main("queued", TaskStatus::Queued);
        let mut in_wait = on_main("in_wait", TaskStatus::Running);
        in_wait.queue.wait = Some(Wait::Dependency {
            titles: vec![format!("{} main (retrying)", BASE_WAIT_TITLE)],
        });
        asking.question = Some(question.clone());
        for other in [&asking, &queued, &in_wait] {
            app.store.save_task(other).unwrap();
        }
        assert!(!moves(&app), "two red tasks must not wait on each other");

        let running = on_main("running", TaskStatus::Running);
        app.store.save_task(&running).unwrap();
        assert!(moves(&app), "a running task can land");
        let mut landing = running.clone();
        landing.status = TaskStatus::Landing;
        app.store.save_task(&landing).unwrap();
        assert!(moves(&app), "a landing task moves the base");
    }

    #[test]
    fn the_policy_budget_is_per_kind_and_cause_not_per_task() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let mut task = task_with_status(TaskStatus::Waiting);
        let spent = Question::new(
            "make verify already fails on base",
            vec![],
            QuestionKind::PreexistingFailure,
            AskedBy::Verify,
        );
        for _ in 0..MAX_POLICY_ANSWERS {
            record_policy_answer(&mut task, &spent, "keep all", "policy", "e", 1);
        }
        assert!(!policy_open(&app, &task, &spent), "that kind is spent");

        // Other kinds still have their own budget, the judge's included.
        let impossible = |n: usize| {
            let mut q = Question::new(
                format!("Criterion {n} cannot be met as written: x. Evidence: y"),
                vec!["retry".into(), "drop criterion".into()],
                QuestionKind::Impossible,
                AskedBy::Implement,
            );
            q.criterion = Some(n);
            q
        };
        assert!(policy_open(&app, &task, &impossible(0)));
        let attempts = Question::new(
            "Attempts keep failing",
            vec![],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        );
        assert!(policy_open(&app, &task, &attempts));

        // One criterion that the judge refuted three times is spent; another is not.
        for _ in 0..MAX_POLICY_ANSWERS {
            record_policy_answer(&mut task, &impossible(0), "retry", "judge", "e", 1);
        }
        assert!(!policy_open(&app, &task, &impossible(0)));
        assert!(policy_open(&app, &task, &impossible(1)));
    }
}
