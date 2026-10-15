pub(in crate::engine) mod authority;
pub(in crate::engine) mod judge;
pub(in crate::engine) mod planner;
pub(in crate::engine) mod publication;

#[cfg(test)]
use self::authority::is_shared_check;
use self::authority::removal_refusal;
#[cfg(test)]
use self::judge::{impossible_parts, parse_judgement, Judgement};
#[cfg(test)]
use crate::engine::decision::impossible_question;
#[cfg(test)]
use crate::engine::verification::base::answers::record_owner_base_check;
#[cfg(test)]
use crate::policy::settings::AfterLand;
#[cfg(test)]
use crate::task::AskedBy;
#[cfg(test)]
use crate::task::CheckSource;
#[cfg(test)]
use crate::task::SourcedCheck;
#[cfg(test)]
use crate::task::VerifyOutcome;
#[cfg(test)]
use crate::task::Wait;
#[cfg(test)]
use crate::verify::repository::record_plan_check_sources;

use crate::clock::now_ms;
use crate::engine::attribution::attributed_target;
use crate::engine::decision::impossible_drop_target;
use crate::engine::lifecycle::base_wait::waits_for_base_move;
#[cfg(test)]
use crate::engine::lifecycle::base_wait::BASE_WAIT_TITLE;
use crate::engine::lines::policy_answer_line;
#[cfg(test)]
use crate::engine::questions::answers::record_answer;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::assert_helper_timeout;
#[cfg(test)]
use crate::engine::test_support::attempt_with_failure;
#[cfg(test)]
use crate::engine::test_support::git_in;
#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::planner_made;
#[cfg(test)]
use crate::engine::test_support::slow_helper_task;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
use crate::harness::cancel::CancelToken;
use crate::policy::questions::limits::hard_limit;
use crate::policy::questions::{cautious_option, owner_only};
use crate::policy::settings::Settings;
use crate::task::checks::authority::{criterion_commands, Origin};
use crate::task::is_option;
use crate::task::AnsweredBy;
use crate::task::AnsweredQuestion;
use crate::task::Assumption;
use crate::task::Question;
use crate::task::QuestionKind;
#[cfg(test)]
use crate::task::Risk;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::PRE_EXISTING_RETRY;
use std::path::Path;
use std::sync::Arc;

/// Automatic answers per kind of question and cause on one task; the next
/// question of that kind and cause goes on to the overseer and the owner.
pub(super) const MAX_POLICY_ANSWERS: usize = 3;

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
    let changed = crate::git::changes::changed_files(Path::new(&task.worktree), &task.base_sha)
        .unwrap_or_default();
    let targeted = question
        .commands
        .iter()
        .map(|command| {
            crate::engine::verification::base::answers::is_task_target(task, command, &changed)
                || attributed_target(app, task, command)
        })
        .collect();
    let may_land_on_default = app.settings.read().unwrap().may_land_on_default(&task.repo);
    let base_moves = task.base_ref.as_ref().is_some_and(|base_ref| {
        // Nothing lands on a default branch that landing may not move, so a
        // wait for it to move would never end.
        if !may_land_on_default
            && crate::git::branches::default_branch(Path::new(&task.repo)).as_deref()
                == Some(base_ref.as_str())
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
                    && crate::git::paths::same_repo_path(&other.repo, &task.repo)
                    && other.base_ref.as_ref() == Some(base_ref)
            })
    });
    (targeted, base_moves)
}

/// The origin of the answer the wait just recorded in the question history
/// (`Owner` also when nothing was recorded).
pub(super) fn origin_of_last_answer(task: &Task) -> Origin {
    match task.question_history.last().map(|h| h.answered_by) {
        Some(AnsweredBy::Owner) | None => Origin::Owner,
        Some(_) => Origin::Other,
    }
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
        if !is_option(
            answer,
            crate::engine::verification::base::answers::PRE_EXISTING_DROP,
        ) {
            return None;
        }
        question.commands.clone()
    } else {
        criterion_commands(task, impossible_drop_target(question, answer)?)
    };
    removal_refusal(settings, task, &commands, Origin::Other)
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
            || (answer == PRE_EXISTING_RETRY
                && crate::engine::verification::base::answers::non_owner_base_retry(task, c, None))
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
                .any(|option| option == crate::engine::verification::base::answers::KEEP_ALL)
            {
                (
                    crate::engine::verification::base::answers::KEEP_ALL,
                    "the subtasks are meant to make the check pass: keep it, once",
                    None,
                )
            } else {
                let all_targeted = !targeted.is_empty() && targeted.iter().all(|value| *value);
                if all_targeted {
                    (
                        crate::engine::verification::base::answers::KEEP_ALL,
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

#[derive(Debug)]
pub(in crate::engine) enum AutomaticAnswer {
    Policy { text: String, evidence: String },
    Judge { text: String, evidence: String },
}

impl AutomaticAnswer {
    fn parts(&self) -> (&str, &'static str, &str) {
        match self {
            Self::Policy { text, evidence } => (text, "policy", evidence),
            Self::Judge { text, evidence } => (text, "judge", evidence),
        }
    }
}

/// Records an automatic answer as an assumption the owner can overturn, with
/// a decision line identifying the rule or judge.
pub(super) fn record_policy_answer(
    task: &mut Task,
    question: &Question,
    answer: &AutomaticAnswer,
    attempt_n: u32,
) {
    let (answer, by, evidence) = answer.parts();
    if question.kind == QuestionKind::AttemptsFailing {
        task.reset_rate_pauses();
    }
    task.record_assumption(Assumption {
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
        task.remember_impossible_answer(question.criterion);
    }
    task.record_decision(policy_answer_line(&question.text, answer, by, evidence));
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
        let by = crate::engine::verification::base::answers::answerer_name(Some(answered_by));
        crate::engine::verification::base::answers::record_base_answer(
            task,
            &question.commands,
            &base_sha,
            by,
            answer,
        );
    }
    // The owner's answers do not count against the orchestrator's continues.
    if question.kind == QuestionKind::AttemptsFailing
        && matches!(
            answered_by,
            AnsweredBy::Orchestrator | AnsweredBy::Overseer | AnsweredBy::Lead
        )
        && !is_option(answer, "stop")
    {
        task.count_non_owner_continue();
    }
    task.record_answer_history(AnsweredQuestion {
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
) -> publication::PolicyResult {
    use publication::{AnswerRuling, PolicyInput, PolicyResult};
    if cancel.is_cancelled() {
        return PolicyResult::Stopped;
    }
    let Some(question) = task.question.clone() else {
        return PolicyResult::Open;
    };
    let planner_check = question.kind == QuestionKind::Impossible
        && question.criterion.is_some_and(|index| {
            planner::planner_check_criterion(&app.settings.read().unwrap(), task, index)
        });
    if !planner_check && !policy_open(app, task, &question) {
        return PolicyResult::Open;
    }
    let Some(input) = PolicyInput::capture(app, task) else {
        return PolicyResult::Stopped;
    };
    let ruling = if planner_check {
        planner::settle_planner_check(app, task, &question, input.attempt, cancel).await
    } else if question.kind == QuestionKind::Impossible {
        judge::judge_impossible(app, task, &question, input.attempt, cancel).await
    } else {
        let (targeted, base_moves) = if question.kind == QuestionKind::PreexistingFailure {
            policy_base_context(app, task, &question)
        } else {
            (Vec::new(), false)
        };
        let Some((text, evidence)) =
            rule_answer_with_context(task, &question, input.attempt, &targeted, base_moves)
        else {
            return PolicyResult::Open;
        };
        AnswerRuling::rule(AutomaticAnswer::Policy { text, evidence })
    };
    match publication::publish(app, task, input, ruling, cancel) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("orch: automatic answer was not published: {error}");
            PolicyResult::Stopped
        }
    }
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
#[path = "answer_policy/tests/callers.rs"]
mod answer_caller_tests;
#[cfg(test)]
#[path = "answer_policy/tests/base_moves.rs"]
mod base_moves_tests;
#[cfg(test)]
#[path = "answer_policy/tests/rules.rs"]
mod tests;
#[cfg(test)]
#[path = "answer_policy/tests/timeout.rs"]
mod timeout_tests;

#[cfg(test)]
#[path = "answer_policy/tests/planner.rs"]
mod planner_check_tests;
