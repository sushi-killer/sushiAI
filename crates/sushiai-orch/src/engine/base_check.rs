use super::*;
use serde::Serialize;

/// Options of the pre-existing final-check question, in the order shown.
pub(super) const PRE_EXISTING_RETRY: &str = "retry after the base is fixed";
pub(super) const PRE_EXISTING_DROP: &str = "drop all";
const PRE_EXISTING_OPTIONS: [&str; 3] = [PRE_EXISTING_RETRY, PRE_EXISTING_DROP, "stop"];

/// Options of the question about a parent's checks that fail on the base:
/// the subtasks are usually meant to make them pass.
pub(super) const KEEP_ALL: &str = "keep all";
const PARENT_CHECK_OPTIONS: [&str; 3] = [KEEP_ALL, PRE_EXISTING_DROP, "stop"];
pub(super) const KEPT_BY_TASK_TARGET: &str = "the task is meant to make it pass";

/// Why a failing check is kept by rule: the files it names do not exist on
/// the base, and the plan creates them.
pub(super) const KEPT_BY_PLAN: &str = "targets absent on base; the plan creates them";
/// Why a failing check is kept by the owner's (or the policy's) `keep all`.
pub(super) const KEPT_BY_ANSWER: &str = "it fails on the base until the subtasks land";

/// The decision line that keeps a check although it fails on the base; it is
/// what stops a relaunch from asking again.
pub(super) fn kept_check_line(command: &str, reason: &str) -> String {
    format!("Orchestrator: kept check {command}, {reason}")
}

/// Whether `task` already keeps `command` although it fails on the base.
pub(super) fn is_kept(task: &Task, command: &str) -> bool {
    [KEPT_BY_PLAN, KEPT_BY_ANSWER, KEPT_BY_TASK_TARGET]
        .iter()
        .any(|reason| task.decisions.contains(&kept_check_line(command, reason)))
}

/// The one owner question for every check that fails on `base_sha`: what
/// each of them printed is quoted, the commands are listed in `commands`.
fn base_check_question(failed: &[VerifyOutcome], base_sha: &str, options: [&str; 3]) -> Question {
    let sha: String = base_sha.chars().take(7).collect();
    let text = match failed {
        [one] => format!(
            "{} already fails on base {sha}: {}",
            one.command,
            tail_chars(one.tail.trim(), 300)
        ),
        many => {
            let names: Vec<&str> = many.iter().map(|f| f.command.as_str()).collect();
            let mut text = format!("{} already fail on base {sha}:", names.join(", "));
            for f in many {
                text.push_str(&format!(
                    "\n- {}: {}",
                    f.command,
                    tail_chars(f.tail.trim(), 200)
                ));
            }
            text
        }
    };
    let mut q = Question::new(
        text,
        options.iter().map(|o| o.to_string()).collect(),
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
    );
    q.commands = failed.iter().map(|f| f.command.clone()).collect();
    q
}

/// The owner question for final checks that fail on the base as well.
pub(super) fn pre_existing_question(failed: &[VerifyOutcome], base_sha: &str) -> Question {
    base_check_question(failed, base_sha, PRE_EXISTING_OPTIONS)
}

/// The owner question for a parent's checks that fail on the base: unlike a
/// final check on its own, a `verify` command is usually meant to turn green
/// through the work.
pub(super) fn parent_checks_question(failed: &[VerifyOutcome], base_sha: &str) -> Question {
    let mut q = base_check_question(failed, base_sha, PARENT_CHECK_OPTIONS);
    let (them, all) = if failed.len() == 1 {
        ("it", "this check")
    } else {
        ("them", "all")
    };
    q.text = format!(
        "{} The subtasks may be meant to make {them} pass: keep {all}, or drop {all}?",
        q.text.trim_end()
    );
    q
}

/// The path-like arguments of a check: a token that holds a `/`, a glob
/// character (`*`, `?`, `[`) or a file extension (`name.ext`). Quotes are
/// stripped; flags, URLs, `VAR=x` assignments, shell operators, absolute
/// paths and bare words (`npm`, `run`, `test`) are left out.
pub(super) fn check_targets(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut redirected = false;
    for token in command.split_whitespace() {
        // The file a `>` writes is not something the check reads.
        if std::mem::replace(&mut redirected, matches!(token, ">" | ">>")) {
            continue;
        }
        let token = token
            .trim_matches(|c| c == '"' || c == '\'')
            .trim_end_matches([';', ',', ')'])
            .trim_start_matches('(')
            .trim_matches(|c| c == '"' || c == '\'');
        let token = path_without_location(token);
        if token.is_empty()
            || token.starts_with(['-', '/', '~', '$', '>', '<', '|', '&'])
            || token.contains("://")
            || token.contains('>')
            || token.contains('<')
            || is_assignment(token)
        {
            continue;
        }
        let has_ext = token.rsplit_once('.').is_some_and(|(name, ext)| {
            !name.is_empty()
                && !name.ends_with('/')
                && ext.len() <= 10
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        });
        if token.contains(['/', '*', '?', '[']) || has_ext {
            let token = token.strip_prefix("./").unwrap_or(token).to_string();
            if !token.is_empty() && !out.contains(&token) {
                out.push(token);
            }
        }
    }
    out
}

fn path_without_location(path: &str) -> &str {
    let mut parts = path.split(':');
    let first = parts.next().unwrap_or(path);
    let line = parts.next();
    if line.is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        && parts
            .next()
            .is_none_or(|n| n.chars().all(|c| c.is_ascii_digit()))
    {
        first
    } else {
        path
    }
}

fn looks_like_identifier_or_path(token: &str) -> bool {
    token.len() >= 6
        && (token.contains('_')
            || token.contains("::")
            || token.contains('/')
            || token.rsplit_once('.').is_some_and(|(name, ext)| {
                !name.is_empty()
                    && !ext.is_empty()
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            }))
}

fn text_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || "`\"'[](){}".contains(c))
        .filter(|token| !token.is_empty())
}

fn output_paths(output: &str) -> Vec<String> {
    output
        .split(|c: char| c.is_whitespace() || "`\"'[](){}<>,;".contains(c))
        .filter_map(|token| {
            let token = token.trim_matches(|c: char| matches!(c, '.' | ':' | ')' | '('));
            let path = path_without_location(token);
            (path != token || check_targets(path).iter().any(|t| t == path))
                .then(|| path.strip_prefix("./").unwrap_or(path).to_string())
        })
        .collect()
}

/// Whether a pre-existing failing check is part of this task's target.
/// `output` is the cached base-run tail, or the tail quoted in the question;
/// changed files are relative to the task's base SHA.
pub(super) fn is_task_target(
    task: &Task,
    command: &str,
    output: &str,
    changed_files: &[String],
) -> bool {
    let flat = super::answer_policy::flat_command(command);
    let named_in_request_or_goal = task
        .request
        .iter()
        .chain(std::iter::once(&task.goal))
        .any(|text| super::answer_policy::flat_command(text).contains(&flat));
    let named_in_criteria = task.criteria.iter().any(|criterion| {
        let flat_criterion = super::answer_policy::flat_command(criterion);
        if !flat_criterion.contains(&flat) {
            return false;
        }
        let Some((prose, gate)) = criterion.split_once("-- check:") else {
            return true;
        };
        let named_in_prose = super::answer_policy::flat_command(prose).contains(&flat);
        let named_only_as_ci_gate = super::answer_policy::runs_ci_entry(command)
            && super::answer_policy::flat_command(gate).contains(&flat)
            && !named_in_prose;
        !named_only_as_ci_gate
    });
    if (named_in_request_or_goal || named_in_criteria)
        && !super::answer_policy::runs_ci_entry(command)
    {
        return true;
    }

    let named_tokens: Vec<String> = task
        .request
        .iter()
        .chain(std::iter::once(&task.goal))
        .chain(task.criteria.iter())
        .flat_map(|text| text_tokens(text))
        .filter(|token| looks_like_identifier_or_path(token))
        .map(str::to_string)
        .collect();
    if named_tokens.iter().any(|token| output.contains(token)) {
        return true;
    }

    let mut paths = check_targets(command);
    paths.extend(output_paths(output));
    paths.iter().any(|path| {
        let path = path_without_location(path);
        changed_files.iter().any(|changed| {
            let changed = changed.strip_prefix("./").unwrap_or(changed);
            changed == path
                || changed.ends_with(&format!("/{path}"))
                || path.ends_with(&format!("/{changed}"))
        })
    })
}

/// Whether `target` (a file, a directory or a glob) names something in
/// `files`, the paths of a tree.
fn target_in_tree(target: &str, files: &[String]) -> bool {
    let dir = format!("{}/", target.trim_end_matches('/'));
    files
        .iter()
        .any(|f| f == target || f.starts_with(&dir) || glob_match(target, f))
}

/// The targets of `command` that are not in the tree of `base_sha`. Read with
/// `git ls-tree`, because the scratch worktree of a base run is gone by now.
/// Empty when the tree cannot be read: nothing is known to be absent.
pub(super) fn targets_absent_on_base(repo: &str, base_sha: &str, command: &str) -> Vec<String> {
    let targets = check_targets(command);
    if targets.is_empty() {
        return Vec::new();
    }
    let Ok(files) = git::tree_files(Path::new(repo), base_sha) else {
        return Vec::new();
    };
    targets
        .into_iter()
        .filter(|t| !target_in_tree(t, &files))
        .collect()
}

/// Whether `target` lies under `path` or matches it: `path` is a file, a
/// directory or a glob of the plan.
fn target_in_plan_path(target: &str, path: &str) -> bool {
    let path = path.strip_prefix("./").unwrap_or(path);
    let dir = format!("{}/", path.trim_end_matches('/'));
    target == path
        || target.starts_with(&dir)
        || glob_match(path, target)
        || glob_match(target, path)
}

/// The failing checks the plan is meant to turn green, kept by rule: a
/// command is kept when at least one of its targets is absent on the base
/// and the task has subtasks, or a target lies under the paths of the task
/// or of a subtask. One decision per kept check is written (once). This is
/// no answer: the answer policy's once-flag and cap do not apply, and no
/// check is removed.
pub(super) fn keep_planned_checks(app: &App, task: &mut Task, failing: &[String]) -> Vec<String> {
    if failing.is_empty() {
        return Vec::new();
    }
    let children: Vec<Task> = app
        .store
        .list_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()) && !t.archived)
        .collect();
    let paths: Vec<&String> = task
        .paths
        .iter()
        .chain(children.iter().flat_map(|c| c.paths.iter()))
        .collect();
    let mut kept = Vec::new();
    for command in failing {
        if targets_absent_on_base(&task.repo, &task.base_sha, command).is_empty() {
            continue;
        }
        let planned = !children.is_empty()
            || check_targets(command)
                .iter()
                .any(|t| paths.iter().any(|p| target_in_plan_path(t, p)));
        if !planned {
            continue;
        }
        let line = kept_check_line(command, KEPT_BY_PLAN);
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
        kept.push(command.clone());
    }
    kept
}

/// Whether `answer` picks `option`: whitespace, trailing punctuation and
/// case do not matter, so `Drop this check.` is `drop this check`.
pub(super) fn is_option(answer: &str, option: &str) -> bool {
    answer
        .trim()
        .trim_end_matches(['.', ',', '!', '?', ';', ':'])
        .trim_end()
        .eq_ignore_ascii_case(option)
}

fn base_check_prefix(who: &str, command: &str, base_sha: &str) -> String {
    let sha: String = base_sha.chars().take(7).collect();
    format!("{who}: base check {command} on {sha} -> ")
}

/// The decision line recording an answer to a base-check question, by `who`
/// (`Owner`, `Lead` or `Orchestrator`).
pub(super) fn base_check_line(who: &str, command: &str, base_sha: &str, answer: &str) -> String {
    format!("{}{answer}", base_check_prefix(who, command, base_sha))
}

/// Whether the owner, lead or orchestrator agent already answered the
/// base-check question for this command on this base.
pub(super) fn base_check_answered(task: &Task, command: &str, base_sha: &str) -> bool {
    let prefixes =
        ["Owner", "Orchestrator", "Lead"].map(|who| base_check_prefix(who, command, base_sha));
    task.decisions
        .iter()
        .any(|d| prefixes.iter().any(|p| d.starts_with(p)))
}

/// Whether a non-owner already retried this command. The RPC gate passes a
/// base SHA; the answer policy checks the command across bases, as its rules do.
pub(super) fn non_owner_base_retry(task: &Task, command: &str, base_sha: Option<&str>) -> bool {
    let decision = task.decisions.iter().any(|d| {
        ["Orchestrator", "Lead"].iter().any(|who| {
            let answer = if let Some(sha) = base_sha {
                d.strip_prefix(&base_check_prefix(who, command, sha))
            } else {
                d.strip_prefix(&format!("{who}: base check {command} on "))
                    .and_then(|rest| rest.split_once(" -> "))
                    .filter(|(sha, _)| {
                        !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit())
                    })
                    .map(|(_, answer)| answer)
            };
            answer.is_some_and(|answer| is_option(answer, PRE_EXISTING_RETRY))
        })
    });
    decision
        || task.assumptions.iter().any(|a| {
            matches!(a.by.as_str(), "policy" | "judge")
                && a.kind == Some(QuestionKind::PreexistingFailure)
                && is_option(&a.answer, PRE_EXISTING_RETRY)
                && a.commands.iter().any(|c| c == command)
                && base_sha.is_none_or(|sha| {
                    let sha: String = sha.chars().take(7).collect();
                    let first_line = a.question.lines().next().unwrap_or_default();
                    first_line.contains(&format!(" already fails on base {sha}:"))
                        || first_line.contains(&format!(" already fail on base {sha}:"))
                })
        })
}

fn base_retry_line(command: &str, base_sha: &str) -> String {
    format!("Orchestrator: retry base check {command} after {base_sha} moves")
}

pub(super) fn record_base_retry(
    task: &mut Task,
    commands: &[String],
    base_sha: &str,
    answer: &str,
) {
    if is_option(answer, PRE_EXISTING_RETRY) {
        for command in commands {
            let line = base_retry_line(command, base_sha);
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }
    }
}

/// A retry remains pending until its exact base moves, including after a
/// restart and when the answer came from the policy.
pub(super) fn base_retry_requested(task: &Task, commands: &[String], base_sha: &str) -> bool {
    commands.iter().any(|command| {
        if task.decisions.contains(&base_retry_line(command, base_sha)) {
            return true;
        }
        let retry_prefix = format!("Orchestrator: retry base check {command} after ");
        if task
            .decisions
            .iter()
            .any(|line| line.starts_with(&retry_prefix))
        {
            return false;
        }
        // Tasks answered before full-SHA retry records existed.
        ["Owner", "Orchestrator"].iter().any(|who| {
            let prefix = base_check_prefix(who, command, base_sha);
            task.decisions.iter().any(|line| {
                line.strip_prefix(&prefix)
                    .is_some_and(|answer| is_option(answer, PRE_EXISTING_RETRY))
            })
        }) || task.assumptions.iter().any(|a| {
            !a.overturned
                && a.kind == Some(QuestionKind::PreexistingFailure)
                && a.commands.contains(command)
                && is_option(&a.answer, PRE_EXISTING_RETRY)
                && a.question.contains(&format!(
                    "base {}:",
                    base_sha.chars().take(7).collect::<String>()
                ))
        })
    })
}

/// Records an answer to a base-check question about `commands`, asked on
/// `base_sha`, one line per command, unless the answer policy gave it (`policy_answered`). The line
/// says who answered, read from the question history entry the wait just
/// wrote: `Owner:`, `Lead:` or `Orchestrator:`.
pub(super) fn record_owner_base_check(
    task: &mut Task,
    commands: &[String],
    base_sha: &str,
    answer: &str,
    policy_answered: bool,
) {
    record_base_retry(task, commands, base_sha, answer);
    if !policy_answered {
        if let Some(who) = answerer_label(task) {
            for command in commands {
                task.decisions
                    .push(base_check_line(who, command, base_sha, answer));
            }
        }
    }
}

/// Who gave the answer the wait just recorded: `Owner` (also when nothing
/// was recorded), `Lead` or `Orchestrator` for an agent, `None` for a rule or judge,
/// whose own line already says so.
pub(super) fn answerer_label(task: &Task) -> Option<&'static str> {
    match task.question_history.last().map(|h| h.answered_by) {
        Some(AnsweredBy::Orchestrator | AnsweredBy::Overseer) => Some("Orchestrator"),
        Some(AnsweredBy::Lead) => Some("Lead"),
        Some(AnsweredBy::Policy | AnsweredBy::Judge) => None,
        Some(AnsweredBy::Owner) | None => Some("Owner"),
    }
}

const BASE_RUNS_FILE: &str = "base-results.json";
/// Most results kept on disk; the rest are dropped when the file is written.
const BASE_RUNS_KEPT: usize = 1000;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBaseRun {
    base_sha: String,
    outcome: VerifyOutcome,
}

fn is_environment_result(outcome: &VerifyOutcome) -> bool {
    let tail = outcome.tail.to_ascii_lowercase();
    ["setup failed", "eexist", "failed to install"]
        .iter()
        .any(|marker| tail.contains(marker))
}

/// The base results of an earlier daemon: (base sha, command) -> outcome.
/// A missing or unreadable file is an empty cache; an entry without an exit
/// code or with an interrupted or environment result is skipped.
pub(super) fn load_base_runs(data_dir: &Path) -> HashMap<(String, String), VerifyOutcome> {
    let rows: Vec<StoredBaseRun> = std::fs::read_to_string(data_dir.join(BASE_RUNS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let poisoned = rows
        .iter()
        .any(|r| is_interrupted(&r.outcome) || is_environment_result(&r.outcome));
    let cache = rows
        .into_iter()
        .filter(|r| {
            r.outcome.code.is_some()
                && !is_interrupted(&r.outcome)
                && !is_environment_result(&r.outcome)
        })
        .map(|r| ((r.base_sha, r.outcome.command.clone()), r.outcome))
        .collect();
    if poisoned {
        save_base_runs(data_dir, &cache);
    }
    cache
}

/// Writes the cache next to the other daemon state (best effort).
fn save_base_runs(data_dir: &Path, cache: &HashMap<(String, String), VerifyOutcome>) {
    let rows: Vec<StoredBaseRun> = cache
        .iter()
        .filter(|(_, o)| o.code.is_some() && !is_interrupted(o) && !is_environment_result(o))
        .take(BASE_RUNS_KEPT)
        .map(|((sha, _), o)| StoredBaseRun {
            base_sha: sha.clone(),
            outcome: o.clone(),
        })
        .collect();
    let (path, tmp) = (
        data_dir.join(BASE_RUNS_FILE),
        data_dir.join(format!("{BASE_RUNS_FILE}.tmp")),
    );
    if let Ok(text) = serde_json::to_string(&rows) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Runs `commands` on the commit `base_sha` of `repo`, in a throwaway
/// detached worktree that is removed again whatever happens (done, failed or
/// cancelled). Results are cached on the app per (base sha, command), so the
/// same command on an unchanged base runs once. Environment results and
/// checkout errors get one retry in a fresh worktree and are never cached.
/// `Err` when neither checkout could be made; nothing is known about the base.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_on_base(
    app: &Arc<App>,
    task_id: Option<&str>,
    repo: &str,
    base_sha: &str,
    commands: &[String],
    run_dir: &Path,
    cancel: &CancelToken,
) -> Result<Vec<VerifyOutcome>, String> {
    let mut known: HashMap<String, VerifyOutcome> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let cache = app.base_runs.lock().unwrap();
        for c in commands {
            match cache.get(&(base_sha.to_string(), c.clone())) {
                Some(r) => {
                    known.insert(c.clone(), r.clone());
                }
                None if !missing.contains(c) => missing.push(c.clone()),
                None => {}
            }
        }
    }
    for attempt in 0..2 {
        if missing.is_empty() || cancel.is_cancelled() {
            break;
        }
        let scratch = app
            .data_dir
            .join("base-runs")
            .join(uuid::Uuid::new_v4().to_string());
        let wt = scratch.join("base");
        let (repo_path, sha, scratch2, wt2) = (
            PathBuf::from(repo),
            base_sha.to_string(),
            scratch.clone(),
            wt.clone(),
        );
        let setup = tokio::task::spawn_blocking(move || -> Result<(), String> {
            std::fs::create_dir_all(&scratch2).map_err(|e| e.to_string())?;
            git::add_detached_worktree(&repo_path, &wt2, &sha).map_err(|e| e.to_string())?;
            git::bootstrap_worktree(&repo_path, &wt2).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let _ = std::fs::create_dir_all(run_dir);
        let sandbox = app.task_sandbox(repo, &wt, run_dir);
        let setup = match setup {
            Ok(()) => {
                super::worktrees::setup_worktree(Path::new(repo), &wt, sandbox.as_ref(), cancel)
                    .await
            }
            err => err,
        };
        let ran = match setup {
            Ok(()) => {
                let results = run_verify_commands(
                    CheckContext::new(app, task_id).with_gate("base checks"),
                    &wt,
                    run_dir,
                    &missing,
                    sandbox.as_ref(),
                    base_sha,
                    cancel,
                )
                .await;
                for result in &results {
                    app.base_run_decisions
                        .lock()
                        .unwrap()
                        .entry((base_sha.to_string(), result.command.clone()))
                        .or_default()
                        .extend(result.decisions.iter().cloned());
                }
                Ok(results)
            }
            Err(e) => Err(format!("could not check out {base_sha}: {e}")),
        };
        let (repo_path, wt3) = (PathBuf::from(repo), wt.clone());
        let _ = tokio::task::spawn_blocking(move || {
            git::remove_worktree(&repo_path, &wt3);
            let _ = std::fs::remove_dir_all(&scratch);
        })
        .await;
        let cancelled = cancel.is_cancelled();
        let results = match ran {
            Ok(results) => results,
            Err(_) if attempt == 0 && !cancelled => continue,
            Err(e) if known.is_empty() => return Err(e),
            Err(_) => break,
        };
        let mut retry = Vec::new();
        let mut cache = app.base_runs.lock().unwrap();
        for (c, r) in missing.iter().zip(results) {
            if is_environment_result(&r) {
                retry.push(c.clone());
                continue;
            }
            // An interrupted run says nothing about the commit.
            if !cancelled && r.code.is_some() && !is_interrupted(&r) {
                cache.insert((base_sha.to_string(), c.clone()), r.clone());
            }
            known.insert(c.clone(), r);
        }
        save_base_runs(&app.data_dir, &cache);
        missing = retry;
    }
    Ok(commands
        .iter()
        .filter_map(|c| known.get(c).cloned())
        .collect())
}

/// The result of `command` on `base_sha`: the cache, else the in-flight
/// background job of the same pair, else a job started now (its run folders
/// under `run_dir`). `None` after environment failures, when the base cannot
/// be checked out or the wait was cancelled: nothing is known then.
/// `background` picks the lane of a job started here (see `start_base_jobs`).
#[allow(clippy::too_many_arguments)]
pub(super) async fn base_result(
    app: &Arc<App>,
    task_id: Option<&str>,
    repo: &str,
    base_sha: &str,
    command: &str,
    run_dir: &Path,
    cancel: &CancelToken,
    background: bool,
) -> Option<VerifyOutcome> {
    start_base_jobs(
        app,
        task_id,
        repo,
        base_sha,
        &[command.to_string()],
        run_dir,
        background,
    );
    wait_for_base(app, base_sha, command, cancel).await
}

/// The base's result for `failed`, when the same command also exits non-zero
/// there: the failure is older than this task. `None` for a command that
/// passes on base or a base that cannot be run. A final check: a job started
/// here runs in the background lane.
pub(super) async fn failing_on_base(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    if is_interrupted(failed) {
        return None;
    }
    let key = (task.base_sha.clone(), failed.command.clone());
    let mut job = app
        .final_base_jobs
        .lock()
        .unwrap()
        .get(&task.id)
        .and_then(|jobs| jobs.get(&key).cloned());
    if job.is_none() {
        let jobs = start_base_jobs(
            app,
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            std::slice::from_ref(&failed.command),
            run_dir,
            true,
        );
        job = jobs.get(&key).cloned();
        app.final_base_jobs
            .lock()
            .unwrap()
            .entry(task.id.clone())
            .or_default()
            .extend(jobs);
    }
    let base = wait_for_base_job(app, &task.base_sha, &failed.command, job, cancel).await;
    record_base_run_decisions(app, task, std::slice::from_ref(&failed.command));
    base.filter(|r| r.code.is_some_and(|c| c != 0) && !is_interrupted(r))
}

/// A `verify` command that failed after the work was carried onto a moved
/// base: `Some` with the base's own result when the command fails on the new
/// base alone yet passed on the base the attempt started from, so the base
/// broke it and the attempt did not. A command that already failed on the
/// old base is a check the work is meant to turn green, never blamed on the
/// base. Cached per base commit like every base run.
pub(super) async fn broken_by_the_new_base(
    app: &Arc<App>,
    task: &Task,
    failed: &VerifyOutcome,
    old_base: &str,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    if is_interrupted(failed) {
        return None;
    }
    // A `verify` command: its lookups use the blocking lane, so a slow
    // background final of the same repo never holds them back.
    let on_new = base_result(
        app,
        Some(&task.id),
        &task.repo,
        &task.base_sha,
        &failed.command,
        &run_dir.join("new"),
        cancel,
        false,
    )
    .await
    .filter(|b| b.code.is_some_and(|c| c != 0) && !is_interrupted(b))?;
    let on_old = base_result(
        app,
        Some(&task.id),
        &task.repo,
        old_base,
        &failed.command,
        &run_dir.join("old"),
        cancel,
        false,
    )
    .await?;
    (on_old.code == Some(0)).then_some(on_new)
}

/// What the owner decided about a `verify` command the new base broke.
pub(super) enum BaseBreak {
    /// The loop was cancelled or stopped while waiting.
    Stopped,
    /// Try again on the newest base: the attempt is not counted.
    Retry,
    /// The check is gone from `verify`; the attempt goes on without it.
    Dropped,
}

/// A `verify` command fails on the new base alone, so the attempt is not to
/// blame: the failure is recorded with a signature of its own and does not
/// count against `attempt_budget`, and the task waits for the owner (the
/// same question a final check that fails on the base gets). The detail
/// tells the next attempt not to fix what is out of its scope.
#[allow(clippy::too_many_arguments)]
pub(super) async fn wait_on_broken_base(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    failed: &VerifyOutcome,
    base_run: &VerifyOutcome,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> BaseBreak {
    let command = failed.command.clone();
    let base_sha = task.base_sha.clone();
    task.decisions.push(format!(
        "Orchestrator: {command} fails on the base {} too",
        short_sha(&base_sha)
    ));
    let code = failed
        .code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "null".to_string());
    record_failure(
        task,
        idx,
        FailureKind::Verify,
        format!(
            "{command} exited {code}, and it fails on the base {} without this task's work: the base broke it, not this attempt. Do not fix it as part of this task.\n{}",
            short_sha(&base_sha),
            failed.tail
        ),
    );
    if let Some(f) = task.attempts[idx].failure.as_mut() {
        f.signature = format!("base:{command}");
    }
    *attempt_budget += 1;
    let assumed = task.assumptions.len();
    let already_retried = base_retry_requested(task, std::slice::from_ref(&command), &base_sha);
    let answer = if already_retried {
        PRE_EXISTING_RETRY.to_string()
    } else {
        task.question = Some(pre_existing_question(
            std::slice::from_ref(base_run),
            &base_sha,
        ));
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        let Some(answer) =
            wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
        else {
            return BaseBreak::Stopped;
        };
        answer
    };
    let policy_answered = task.assumptions.len() > assumed;
    if !already_retried {
        record_owner_base_check(
            task,
            std::slice::from_ref(&command),
            &base_sha,
            &answer,
            policy_answered,
        );
    }
    if is_option(&answer, PRE_EXISTING_DROP) {
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        match remove_check_command(&settings, task, &command, origin) {
            Ok(()) => {
                task.decisions
                    .push(format!("Orchestrator: dropped check {command}"));
                app.verify_cache.lock().unwrap().remove(task_id);
                *attempt_budget -= 1;
                let a = &mut task.attempts[idx];
                a.status = AttemptStatus::Running;
                a.ended_at = None;
                a.failure = None;
                task.status = TaskStatus::Running;
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                return BaseBreak::Dropped;
            }
            // Refused: the check stays and the attempt stays failed. The
            // answer counts as a retry on the newest base.
            Err(why) => task
                .decisions
                .push(format!("Orchestrator: check {command} stays: {why}")),
        }
    }
    let wt = PathBuf::from(&task.worktree);
    match carry_after_base_moves(app, task, &wt, pending_answer, cancel, permit).await {
        BaseRetry::Ready => {}
        BaseRetry::Conflict(detail) => record_failure(task, idx, FailureKind::Conflict, detail),
        BaseRetry::Stopped => return BaseBreak::Stopped,
    }
    task.updated_at = now_ms();
    app.save_task_logged(task);
    app.broadcast_task(task);
    BaseBreak::Retry
}

/// Waits for the turn of `repo`'s base-check pairs; `None` when cancelled.
/// Two lanes per repo: blocking runs (a parent's `verify`, a carried
/// attempt's lookups) and background `finalVerify` jobs. Each lane runs one
/// pair at a time; a slow final never queues a blocking run behind it.
async fn lock_base_checks(
    app: &App,
    repo: &str,
    background: bool,
    cancel: &CancelToken,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    let key = if background {
        format!("{repo}\u{0}background")
    } else {
        repo.to_string()
    };
    let lock = app
        .base_check_locks
        .lock()
        .unwrap()
        .entry(key)
        .or_default()
        .clone();
    tokio::select! {
        guard = lock.lock_owned() => Some(guard),
        _ = cancel.cancelled() => None,
    }
}

pub(super) type BaseJobReceivers = HashMap<(String, String), watch::Receiver<bool>>;

/// Starts one background job for the pairs of `commands` on `base_sha` that
/// are neither cached nor in flight; their run folders go under `run_dir`
/// (`preflight-base`, `preflight-base-retry`). The job is the run plus the
/// rerun of a command that exits non-zero, past the cache and with no other
/// base-check pair of its repo beside it, so a load-flaky test does not
/// count. It runs under the app's own `base_cancel` (a task stop does not
/// end it: another task may wait on the same job), writes its results into
/// `base_runs` and leaves `base_jobs` when done. A cancelled job caches
/// nothing. `background` jobs (finals) take the repo's background lane,
/// the others the blocking lane. Returns receipts for new and in-flight jobs.
pub(super) fn start_base_jobs(
    app: &Arc<App>,
    task_id: Option<&str>,
    repo: &str,
    base_sha: &str,
    commands: &[String],
    run_dir: &Path,
    background: bool,
) -> BaseJobReceivers {
    let mut todo: Vec<String> = Vec::new();
    let mut receipts = HashMap::new();
    let (tx, rx) = watch::channel(false);
    {
        // Jobs first, then the cache: a job caches before it leaves the
        // registry, so a pair is never missed in between.
        let mut jobs = app.base_jobs.lock().unwrap();
        let cache = app.base_runs.lock().unwrap();
        for c in commands {
            let key = (base_sha.to_string(), c.clone());
            if let Some(job) = jobs.get(&key) {
                receipts.insert(key, job.clone());
                continue;
            }
            if cache.contains_key(&key) {
                continue;
            }
            receipts.insert(key.clone(), rx.clone());
            jobs.insert(key, rx.clone());
            todo.push(c.clone());
        }
    }
    if todo.is_empty() {
        return receipts;
    }
    let task_id = task_id.map(str::to_string);
    let (app, repo, base_sha, run_dir) = (
        app.clone(),
        repo.to_string(),
        base_sha.to_string(),
        run_dir.to_path_buf(),
    );
    // Leaves the registry and wakes the waiters even when the job panics.
    struct Leave {
        app: Arc<App>,
        base_sha: String,
        todo: Vec<String>,
        tx: watch::Sender<bool>,
    }
    impl Drop for Leave {
        fn drop(&mut self) {
            if self.app.base_cancel.is_cancelled() {
                let mut cache = self.app.base_runs.lock().unwrap();
                for c in &self.todo {
                    cache.remove(&(self.base_sha.clone(), c.clone()));
                }
            }
            let mut jobs = self.app.base_jobs.lock().unwrap();
            for c in &self.todo {
                jobs.remove(&(self.base_sha.clone(), c.clone()));
            }
            drop(jobs);
            let _ = self.tx.send(true);
        }
    }
    tokio::spawn(async move {
        let _leave = Leave {
            app: app.clone(),
            base_sha: base_sha.clone(),
            todo: todo.clone(),
            tx,
        };
        run_base_job(
            &app,
            task_id.as_deref(),
            &repo,
            &base_sha,
            &todo,
            &run_dir,
            background,
        )
        .await;
    });
    receipts
}

async fn run_base_job(
    app: &Arc<App>,
    task_id: Option<&str>,
    repo: &str,
    base_sha: &str,
    commands: &[String],
    dir: &Path,
    background: bool,
) {
    let cancel = app.base_cancel.clone();
    let Some(_alone) = lock_base_checks(app, repo, background, &cancel).await else {
        return;
    };
    let fails = |r: &VerifyOutcome| r.code.is_some_and(|c| c != 0) && !is_interrupted(r);
    let Ok(first) = run_on_base(
        app,
        task_id,
        repo,
        base_sha,
        commands,
        &dir.join("preflight-base"),
        &cancel,
    )
    .await
    else {
        return;
    };
    let failed: Vec<String> = first
        .iter()
        .filter(|r| fails(r))
        .map(|r| r.command.clone())
        .collect();
    if failed.is_empty() || cancel.is_cancelled() {
        return;
    }
    {
        let mut cache = app.base_runs.lock().unwrap();
        for c in &failed {
            cache.remove(&(base_sha.to_string(), c.clone()));
        }
        save_base_runs(&app.data_dir, &cache);
    }
    let Ok(second) = run_on_base(
        app,
        task_id,
        repo,
        base_sha,
        &failed,
        &dir.join("preflight-base-retry"),
        &cancel,
    )
    .await
    else {
        return;
    };
    for c in &failed {
        if second
            .iter()
            .any(|r| &r.command == c && r.code == Some(0) && !is_interrupted(r))
        {
            app.base_flaky
                .lock()
                .unwrap()
                .insert((base_sha.to_string(), c.clone()));
            let line = flaky_base_check_line(c, base_sha);
            app.base_run_decisions
                .lock()
                .unwrap()
                .entry((base_sha.to_string(), c.clone()))
                .or_default()
                .push(line.clone());
            CheckContext::new(app, task_id).decision(line, &mut Vec::new());
        }
    }
}

/// Waits for the in-flight job of the pair (if any), then reads the cache.
/// `None`: the job was cancelled, hit an environment failure or could not
/// check out the base, or `cancel` fired while waiting.
async fn wait_for_base(
    app: &Arc<App>,
    base_sha: &str,
    command: &str,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let key = (base_sha.to_string(), command.to_string());
    let job = app.base_jobs.lock().unwrap().get(&key).cloned();
    wait_for_base_job(app, base_sha, command, job, cancel).await
}

async fn wait_for_base_job(
    app: &Arc<App>,
    base_sha: &str,
    command: &str,
    job: Option<watch::Receiver<bool>>,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    if let Some(mut rx) = job {
        tokio::select! {
            // An `Err` is a dropped sender: the job is over either way.
            _ = rx.wait_for(|done| *done) => {}
            _ = cancel.cancelled() => return None,
        }
    }
    app.base_runs
        .lock()
        .unwrap()
        .get(&(base_sha.to_string(), command.to_string()))
        .cloned()
}

/// Records on `task` the commands whose first base run failed and whose
/// rerun passed: `base check <cmd> flaky on <sha>: passed on rerun`.
pub(super) fn note_flaky_base_checks(app: &App, task: &mut Task, commands: &[String]) {
    let flaky = app.base_flaky.lock().unwrap();
    for c in commands {
        if !flaky.contains(&(task.base_sha.clone(), c.clone())) {
            continue;
        }
        let line = flaky_base_check_line(c, &task.base_sha);
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
    }
}

fn flaky_base_check_line(command: &str, base_sha: &str) -> String {
    format!(
        "Orchestrator: base check {command} flaky on {}: passed on rerun",
        short_sha(base_sha)
    )
}

/// Runs `checks` on the task's base, waiting for the result: every command
/// whose pair is not cached starts a background job (or joins the one in
/// flight). A command that exits non-zero is run once more by the job (the
/// task records `base check <cmd> flaky on <sha>: passed on rerun`); every
/// one that fails both times is returned. Empty when every command passes,
/// the base cannot be checked out, or the run was cancelled.
///
/// A parent passes its unkept `verify` commands here and asks first: a
/// `verify` command is usually the very test the work is meant to turn
/// green, so failing on the base is what it is for.
pub(super) async fn failing_twice_on_base(
    app: &Arc<App>,
    task: &mut Task,
    checks: &[String],
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let commands = planned_base_commands(app, task, checks);
    if commands.is_empty() {
        return Vec::new();
    }
    let dir = app.store.task_dir(&task.id);
    start_base_jobs(
        app,
        Some(&task.id),
        &task.repo,
        &task.base_sha,
        &commands,
        &dir,
        false,
    );
    let mut failing = Vec::new();
    for c in &commands {
        if let Some(r) = wait_for_base(app, &task.base_sha, c, cancel).await {
            record_check_outcomes(app, task, std::slice::from_ref(&r));
            if r.code.is_some_and(|code| code != 0) && !is_interrupted(&r) {
                failing.push(r);
            }
        }
    }
    if cancel.is_cancelled() {
        return Vec::new();
    }
    record_base_run_decisions(app, task, &commands);
    note_flaky_base_checks(app, task, &commands);
    failing
}

fn record_base_run_decisions(app: &App, task: &mut Task, commands: &[String]) {
    let decisions = app.base_run_decisions.lock().unwrap();
    let mut lines = Vec::new();
    for command in commands {
        if let Some(recorded) = decisions.get(&(task.base_sha.clone(), command.clone())) {
            for line in recorded {
                let line = if let Some(held) = task.held_out.as_ref().filter(|h| &h.run == command)
                {
                    line.replace(command, &brief::held_out_label(held.criterion))
                } else {
                    line.clone()
                };
                lines.push(line);
            }
        }
    }
    record_check_decisions(task, lines);
}

/// `checks` without duplicates, minus what the scoped-checks policy skips
/// for the task's planned paths (no diff yet: they stand in for it).
fn planned_base_commands(app: &App, task: &Task, checks: &[String]) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for c in checks {
        if !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    filter_scoped_planned(
        &scoped_checks_for(&app.settings.read().unwrap(), task),
        &task.repo,
        &commands,
        &task.paths,
    )
}

/// Starts the background base jobs of the task's `finalVerify` that are not
/// answered yet; the first attempt does not wait for them.
pub(super) fn start_final_base_jobs(app: &Arc<App>, task: &Task) {
    // A command deferred to the parent does not run here, so it has no base
    // run here either.
    let (finals, _) = {
        let settings = app.settings.read().unwrap().clone();
        split_deferred(app, &settings, task)
    };
    let unanswered: Vec<String> = finals
        .into_iter()
        .filter(|c| !is_kept(task, c) && !base_check_answered(task, c, &task.base_sha))
        .collect();
    let commands = planned_base_commands(app, task, &unanswered);
    if commands.is_empty() {
        return;
    }
    let dir = app.store.task_dir(&task.id);
    let jobs = start_base_jobs(
        app,
        Some(&task.id),
        &task.repo,
        &task.base_sha,
        &commands,
        &dir,
        true,
    );
    app.final_base_jobs
        .lock()
        .unwrap()
        .entry(task.id.clone())
        .or_default()
        .extend(jobs);
}

/// Baselines the checks that have none yet on the base commit, or on the
/// planner's tests commit when the task has one (not in the
/// task's worktree, which may hold the implementer's changes). `false` when
/// cancelled or the base could not be checked out; nothing is stored then.
pub(super) async fn baseline_on_base(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> bool {
    let todo = baseline_todo(task);
    if todo.is_empty() {
        return true;
    }
    let commands: Vec<String> = todo.iter().map(|(_, _, run)| run.clone()).collect();
    let run_dir = app.store.task_dir(&task.id).join("baseline-base");
    let rev = task.work_base().to_string();
    match run_on_base(
        app,
        Some(&task.id),
        &task.repo,
        &rev,
        &commands,
        &run_dir,
        cancel,
    )
    .await
    {
        Ok(results) if results.len() == todo.len() && !cancel.is_cancelled() => {
            record_check_outcomes(app, task, &results);
            record_base_run_decisions(app, task, &commands);
            record_baselines(task, &todo, &results)
        }
        _ => false,
    }
}

/// What `task.amend` changes; a `None` field is left as it is.
#[derive(Debug, Clone, Default)]
pub(super) struct Amendment {
    pub criteria: Option<Vec<String>>,
    /// The `criteria` texts flagged visual; only meaningful when `criteria` is set.
    pub visual_criteria: Vec<String>,
    pub verify: Option<Vec<String>>,
    pub final_verify: Option<Vec<String>>,
    /// The screenshot command; an empty string removes it.
    pub screenshot: Option<String>,
    pub checks: Option<Vec<Check>>,
    pub held_out: Option<Option<Check>>,
    pub depends_on: Option<Vec<String>>,
    pub dependency_titles: Vec<String>,
}

impl Amendment {
    pub fn is_empty(&self) -> bool {
        self.fields().is_empty()
    }

    /// The camelCase names of the fields it changes, in a fixed order.
    pub fn fields(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.criteria.is_some() {
            names.push("criteria");
        }
        if self.verify.is_some() {
            names.push("verify");
        }
        if self.final_verify.is_some() {
            names.push("finalVerify");
        }
        if self.screenshot.is_some() {
            names.push("screenshot");
        }
        if self.checks.is_some() {
            names.push("checks");
        }
        if self.held_out.is_some() {
            names.push("heldOut");
        }
        if self.depends_on.is_some() {
            names.push("dependsOn");
        }
        names
    }

    /// A later amendment wins field by field.
    pub fn merge(&mut self, later: Amendment) {
        if later.criteria.is_some() {
            self.criteria = later.criteria;
            self.visual_criteria = later.visual_criteria;
        }
        self.verify = later.verify.or(self.verify.take());
        self.final_verify = later.final_verify.or(self.final_verify.take());
        self.screenshot = later.screenshot.or(self.screenshot.take());
        self.checks = later.checks.or(self.checks.take());
        self.held_out = later.held_out.or(self.held_out.take());
        if later.depends_on.is_some() {
            self.depends_on = later.depends_on;
            self.dependency_titles = later.dependency_titles;
        }
    }

    /// Replaces the fields and records their names, plus dependency titles.
    pub fn apply(self, task: &mut Task) {
        let line = format!("Amended: {}", self.fields().join(", "));
        if let Some(v) = self.criteria {
            task.criteria = v;
            task.visual_criteria = self.visual_criteria;
        }
        if let Some(v) = self.verify {
            task.verify = v;
        }
        if let Some(v) = self.final_verify {
            task.final_verify = v;
        }
        if let Some(v) = self.screenshot {
            let v = v.trim();
            task.brief_check.screenshot = (!v.is_empty()).then(|| v.to_string());
        }
        let len = task.criteria.len();
        if let Some(v) = self.checks {
            task.checks = valid_checks(v, len);
        }
        if let Some(h) = self.held_out {
            task.held_out = valid_check(h, len);
        }
        task.decisions.push(line);
        if let Some(deps) = self.depends_on {
            task.depends_on = deps;
            let waits = if self.dependency_titles.is_empty() {
                "nothing".to_string()
            } else {
                self.dependency_titles
                    .iter()
                    .map(|title| format!("\"{title}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            task.decisions
                .push(format!("Dependencies: waits for {waits}"));
        }
        task.updated_at = now_ms();
    }
}

/// Hands a pending amendment to the loop's own copy of the task: applied,
/// baselined on the base commit with `variant.groundedChecks`, and saved.
/// Called only at attempt boundaries, never while a harness runs.
pub(super) async fn apply_pending_amendment(
    app: &Arc<App>,
    task: &mut Task,
    pending: &Arc<StdMutex<Option<Amendment>>>,
    cancel: &CancelToken,
) {
    let Some(amendment) = pending.lock().unwrap().take() else {
        return;
    };
    if amendment.verify.is_some() {
        // Cached results belong to the old commands.
        app.verify_cache.lock().unwrap().remove(&task.id);
    }
    amendment.apply(task);
    baseline_on_base(app, task, cancel).await;
    app.save_task_logged(task);
    app.broadcast_task(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(command: &str, code: i32) -> VerifyOutcome {
        VerifyOutcome {
            command: command.to_string(),
            code: Some(code),
            tail: String::new(),
            ms: 1,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_flaky_background_base_check_records_its_decision_after_the_task_finishes() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Done);
        task.repo = repo.to_str().unwrap().into();
        task.base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
        let counter = dir.path().join("counter");
        let command = format!(
            "echo run >> '{}'; test \"$(wc -l < '{}')\" -ge 2",
            counter.display(),
            counter.display()
        );
        app.store.save_task(&task).unwrap();

        run_base_job(
            &app,
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            std::slice::from_ref(&command),
            &dir.path().join("runs"),
            true,
        )
        .await;

        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Done);
        assert!(saved.decisions.contains(&format!(
            "Orchestrator: base check {command} flaky on {}: passed on rerun",
            short_sha(&task.base_sha)
        )));
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 2);
        assert_eq!(
            app.base_runs.lock().unwrap()[&(task.base_sha.clone(), command)].code,
            Some(0)
        );
    }

    #[tokio::test]
    async fn an_interrupted_base_run_is_not_cached() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base = git_in(&repo, &["rev-parse", "HEAD"]);
        let counter = dir.path().join("counter");
        let command = format!(
            "echo run >> '{}'; n=$(wc -l < '{}'); if [ \"$n\" -le 2 ]; then exit 137; fi",
            counter.display(),
            counter.display()
        );
        let commands = vec![command.clone()];
        let cancel = CancelToken::new();
        let run_dir = dir.path().join("runs");
        let first = run_on_base(
            &app,
            None,
            repo.to_str().unwrap(),
            &base,
            &commands,
            &run_dir,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(first[0].code, Some(137));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        assert!(app.base_runs.lock().unwrap().is_empty());
        assert!(load_base_runs(&app.data_dir).is_empty());
        let second = run_on_base(
            &app,
            None,
            repo.to_str().unwrap(),
            &base,
            &commands,
            &run_dir,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(second[0].code, Some(0));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
        assert_eq!(app.base_runs.lock().unwrap().len(), 1);
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_str().unwrap().into();
        task.base_sha = base.clone();
        assert!(failing_twice_on_base(&app, &mut task, &commands, &cancel)
            .await
            .is_empty());
        assert!(task
            .decisions
            .iter()
            .any(|l| l.contains("was interrupted") && l.contains("after") && l.contains(" s")));
        let interrupted = [124, 137, 143]
            .into_iter()
            .map(|code| StoredBaseRun {
                base_sha: base.clone(),
                outcome: outcome(&format!("exit {code}"), code),
            })
            .collect::<Vec<_>>();
        std::fs::write(
            app.data_dir.join(BASE_RUNS_FILE),
            serde_json::to_string(&interrupted).unwrap(),
        )
        .unwrap();
        assert!(load_base_runs(&app.data_dir).is_empty());
    }

    #[tokio::test]
    async fn a_pair_in_flight_is_waited_for_and_never_started_twice() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let key = ("abc".to_string(), "npm run ci".to_string());
        let (tx, rx) = watch::channel(false);
        app.base_jobs.lock().unwrap().insert(key.clone(), rx);
        let cancel = CancelToken::new();
        let waiter = {
            let app = app.clone();
            let run_dir = app.data_dir.join("runs");
            tokio::spawn(async move {
                base_result(
                    &app,
                    None,
                    "/no/such/repo",
                    "abc",
                    "npm run ci",
                    &run_dir,
                    &cancel,
                    true,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished(), "it waits for the job");
        assert_eq!(app.base_jobs.lock().unwrap().len(), 1, "no second job");
        // The job ends: its result is cached first, then it leaves the registry.
        app.base_runs
            .lock()
            .unwrap()
            .insert(key.clone(), outcome("npm run ci", 1));
        app.base_jobs.lock().unwrap().remove(&key);
        tx.send(true).unwrap();
        let got = waiter.await.unwrap().expect("the job's result");
        assert_eq!(got.code, Some(1));
        assert!(app.base_jobs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_cancelled_job_caches_nothing_and_its_waiters_get_base_unknown() {
        let (app, dir) = crate::engine::test_support::test_app();
        app.base_cancel.cancel();
        let run_dir = dir.path().join("runs");
        let got = base_result(
            &app,
            None,
            "/no/such/repo",
            "abc",
            "npm run ci",
            &run_dir,
            &CancelToken::new(),
            true,
        )
        .await;
        assert!(got.is_none());
        assert!(app.base_runs.lock().unwrap().is_empty());
        assert!(app.base_jobs.lock().unwrap().is_empty());
        let left = std::fs::read_dir(dir.path().join("data/base-runs")).map_or(0, |d| d.count());
        assert_eq!(left, 0, "no scratch worktree is left");
    }

    #[tokio::test]
    async fn base_check_pairs_of_one_repo_run_one_at_a_time() {
        let (app, dir) = crate::engine::test_support::test_app();
        let task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let lock = app
            .base_check_locks
            .lock()
            .unwrap()
            .entry(task.repo.clone())
            .or_default()
            .clone();
        let _busy = lock.lock().await;
        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        // Another pair of the same repo is running: this one waits for it
        // (here until it is cancelled) instead of running beside it.
        let mut task = task;
        let failed = failing_twice_on_base(&app, &mut task, &["true".to_string()], &cancel).await;
        assert!(failed.is_empty());
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(
            !dir.path().join("data/base-runs").exists(),
            "it never started"
        );
    }

    #[tokio::test]
    async fn a_refused_drop_leaves_the_check_and_the_failed_attempt_and_retries() {
        let (app, dir) = crate::engine::test_support::test_app();
        app.settings.write().unwrap().answer_policy = false;
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        crate::engine::test_support::init_git_repo(&repo);
        let wt = dir.path().join("worktree");
        let created = git::create_worktree(&repo, &task.branch, &wt, "HEAD").unwrap();
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha;
        task.base_ref = git::branch_of(&repo, "HEAD");
        crate::engine::test_support::git_in(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "fix base"],
        );
        let new_base = git::head_sha(&repo).unwrap();
        task.verify = vec!["npm run ci".into()];
        task.attempts = vec![crate::engine::test_support::attempt_with_failure(1, "x")];
        task.attempts[0].status = AttemptStatus::Failed;
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let failed = VerifyOutcome {
            command: "npm run ci".into(),
            code: Some(1),
            tail: "boom".into(),
            ms: 1,
            ..Default::default()
        };
        let pending: Arc<StdMutex<Option<oneshot::Sender<Reply>>>> = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let mut budget = 3u32;
        let mut permit = None;
        // An agent's "drop all" that got past the RPC gate (the
        // setting changed in between, say) is still refused by the loop.
        let deliver = async {
            loop {
                let tx = pending.lock().unwrap().take();
                if let Some(tx) = tx {
                    app.agent_answers
                        .lock()
                        .unwrap()
                        .insert(id.clone(), AnsweredBy::Orchestrator);
                    tx.send(Reply {
                        text: PRE_EXISTING_DROP.to_string(),
                        default_evidence: None,
                    })
                    .ok();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let (end, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                wait_on_broken_base(
                    &app,
                    &id,
                    &mut task,
                    0,
                    &failed,
                    &failed,
                    &mut budget,
                    &pending,
                    &cancel,
                    &mut permit,
                ),
                deliver
            )
        })
        .await
        .expect("a refused drop must retry on the moved base without another owner answer");
        assert!(matches!(end, BaseBreak::Retry));
        assert_eq!(task.base_sha, new_base);
        assert_eq!(git::head_sha(&wt).unwrap(), new_base);
        assert!(task.question.is_none());
        assert_eq!(task.verify, vec!["npm run ci".to_string()]);
        assert_eq!(task.attempts[0].status, AttemptStatus::Failed);
        assert!(task.attempts[0].failure.is_some());
        assert_ne!(task.status, TaskStatus::Running);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("check npm run ci stays")));
    }

    #[test]
    fn a_failing_final_whose_missing_target_the_plan_creates_is_kept_by_rule() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        let sha = git(&["rev-parse", "HEAD"]);
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.repo = repo.path().to_string_lossy().to_string();
        task.base_sha = sha;
        task.paths = vec!["tests/new_check.sh".to_string()];
        let failing = vec![
            "sh tests/new_check.sh".to_string(),
            "sh tests/other.sh".to_string(),
        ];
        let kept = keep_planned_checks(&app, &mut task, &failing);
        assert_eq!(kept, vec!["sh tests/new_check.sh".to_string()]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("sh tests/new_check.sh")));
    }

    #[test]
    fn options_match_ignoring_case_space_and_trailing_punctuation() {
        assert!(is_option("Drop all.", PRE_EXISTING_DROP));
        assert!(is_option("  DROP ALL! ", PRE_EXISTING_DROP));
        assert!(is_option("drop all", PRE_EXISTING_DROP));
        assert!(is_option("Stop?!", "stop"));
        assert!(!is_option("drop all please", PRE_EXISTING_DROP));
        assert!(!is_option("keep all", PRE_EXISTING_DROP));
    }

    #[test]
    fn an_answered_pair_is_found_by_command_and_short_base_sha() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        assert!(!base_check_answered(&task, "npm test", sha));
        let cmds = ["npm test".to_string()];
        record_owner_base_check(&mut task, &cmds, sha, "drop all", true);
        assert!(!base_check_answered(&task, "npm test", sha));
        record_owner_base_check(&mut task, &cmds, sha, "drop all", false);
        assert_eq!(
            task.decisions.last().unwrap(),
            "Owner: base check npm test on 0123456 -> drop all"
        );
        assert!(base_check_answered(&task, "npm test", sha));
        assert!(base_check_answered(&task, "npm test", "0123456ffff"));
        assert!(!base_check_answered(&task, "npm test", "fedcba9876"));
        assert!(!base_check_answered(&task, "npm run lint", sha));
    }

    #[test]
    fn an_orchestrator_answer_is_labelled_so_and_not_asked_again() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        let cmds = ["npm test".to_string()];
        let failed = VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "tail".into(),
            ms: 1,
            ..Default::default()
        };
        let q = pre_existing_question(&[failed], sha);
        crate::engine::answer_policy::record_answered_question(
            &mut task,
            &q,
            "keep all",
            AnsweredBy::Orchestrator,
        );
        record_owner_base_check(&mut task, &cmds, sha, "keep all", false);
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert_eq!(
            task.decisions.last().unwrap(),
            "Orchestrator: base check npm test on 0123456 -> keep all"
        );
        assert!(base_check_answered(&task, "npm test", sha));
    }

    #[test]
    fn every_answerer_records_a_retry_on_the_exact_base() {
        let sha = "0123456789abcdef";
        let commands = ["npm test".to_string()];
        for who in [
            AnsweredBy::Owner,
            AnsweredBy::Orchestrator,
            AnsweredBy::Policy,
        ] {
            let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
            task.base_sha = sha.to_string();
            let question = pre_existing_question(
                &[VerifyOutcome {
                    command: commands[0].clone(),
                    code: Some(1),
                    tail: "failed".to_string(),
                    ms: 1,
                    ..Default::default()
                }],
                sha,
            );
            record_answered_question(&mut task, &question, PRE_EXISTING_RETRY, who);
            assert!(base_retry_requested(&task, &commands, sha));
            assert!(!base_retry_requested(&task, &commands, "0123456ffff"));
            assert!(!base_retry_requested(
                &task,
                &["npm run lint".to_string()],
                sha
            ));
        }
    }

    #[test]
    fn check_targets_finds_files_and_globs_only() {
        let t = |c: &str| check_targets(c);
        assert_eq!(
            t("node --test tests/a.test.cjs 'tests/b/*.cjs' \"src/c.ts\""),
            ["tests/a.test.cjs", "tests/b/*.cjs", "src/c.ts"]
        );
        assert_eq!(
            t("test -f ./Cargo.toml && grep -q x docs/[a-z]?.md"),
            ["Cargo.toml", "docs/[a-z]?.md"]
        );
        assert_eq!(t("cat build.log | wc -l > out.txt"), ["build.log"]);
        // Left out: flags, URLs, assignments, operators, bare words, versions.
        assert!(t("npm run test -- --reporter=dot -x").is_empty());
        assert!(t("cargo test -p sushiai-orch --test graph").is_empty());
        assert!(t("curl -fsS https://example.com/a/b.json").is_empty());
        assert!(t("CI=1 FILE=src/a.rs make check").is_empty());
        assert!(t("true && false || echo ok ; ls | cat < /dev/null").is_empty());
        assert!(t("node --version 18.2 2>/dev/null").is_empty());
    }

    #[test]
    fn a_target_is_present_by_path_directory_or_glob() {
        let files: Vec<String> = ["src/a.rs", "docs/guide/x.md"].map(String::from).to_vec();
        assert!(target_in_tree("src/a.rs", &files));
        assert!(target_in_tree("docs/guide", &files));
        assert!(target_in_tree("docs/guide/", &files));
        assert!(target_in_tree("src/*.rs", &files));
        assert!(!target_in_tree("src/b.rs", &files));
        assert!(!target_in_tree("docs/gui", &files));
        assert!(target_in_plan_path("src/new/a.rs", "src/new"));
        assert!(target_in_plan_path("src/new/a.rs", "src/**"));
        assert!(target_in_plan_path("src/new/*.rs", "src/new/a.rs"));
        assert!(!target_in_plan_path("tests/a.rs", "src/new"));
    }

    #[test]
    fn task_target_detection_uses_request_output_and_changed_paths_but_ignores_ci_gates() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        task.request = Some("Fix a_base_retry_carries_the_planner_test_commit".into());
        assert!(is_task_target(
            &task,
            "cargo test -p sushiai-orch --lib",
            "test a_base_retry_carries_the_planner_test_commit ... FAILED",
            &[]
        ));

        task.request = None;
        assert!(is_task_target(
            &task,
            "cargo test -p sushiai-orch",
            "error in crates/sushiai-orch/tests/base_retry.rs:17:2",
            &["crates/sushiai-orch/tests/base_retry.rs".into()]
        ));

        task.criteria = vec!["Checks pass -- check: npm run ci".into()];
        assert!(!is_task_target(&task, "npm run ci", "ci failed", &[]));
        task.criteria =
            vec!["Run the focused test -- check: node --test tests/base_retry.test.cjs".into()];
        assert!(is_task_target(
            &task,
            "node --test tests/base_retry.test.cjs",
            "test failed",
            &[]
        ));
        task.criteria = vec!["Checks pass -- check: npm run ci".into()];
        task.goal = "Fix the failing a_base_retry_carries_the_planner_test_commit test".into();
        assert!(is_task_target(
            &task,
            "npm run ci",
            "a_base_retry_carries_the_planner_test_commit ... FAILED",
            &[]
        ));
    }

    #[test]
    fn a_task_target_keep_decision_survives_later_base_checks() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        task.decisions
            .push(kept_check_line("cargo test", KEPT_BY_TASK_TARGET));
        assert!(is_kept(&task, "cargo test"));
        assert!(!is_kept(&task, "npm test"));
    }
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    #[test]
    fn a_retry_of_a_longer_command_does_not_cover_its_prefix_command() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.decisions.push(base_check_line(
            "Lead",
            "check on target",
            &task.base_sha,
            PRE_EXISTING_RETRY,
        ));
        assert!(non_owner_base_retry(&task, "check on target", None));
        assert!(!non_owner_base_retry(&task, "check", None));
        assert!(!non_owner_base_retry(&task, "check", Some(&task.base_sha)));
    }
}
