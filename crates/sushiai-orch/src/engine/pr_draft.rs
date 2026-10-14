//! A done top-level task whose work did not land on its base gets a pull
//! request, with no help from the overseer. One read-only helper drafts the
//! title and body by the repo's own rules; a second checks the body against
//! the final diff and the check results before anything is pushed. orchd then
//! opens it through `task.pr`, on the task's own branch. A transient failure
//! (network, `gh`) is retried with a growing delay; an auth or permanent
//! failure sends the owner one notice. The run holds no slot, never blocks the
//! task loop and never changes the task's status: it stays done, and `task.pr`
//! can open the pull request later.

use super::*;

/// Longest diff given to the drafter, in characters.
const MAX_DIFF_CHARS: usize = 20_000;
/// Longest pull request body sent to GitHub, in characters.
const MAX_BODY_CHARS: usize = 20_000;

/// The reply shape the drafter must match, enforced by the harness
/// (`--json-schema` / `--output-schema`).
const DRAFT_SCHEMA: &str = r#"{"type":"object","properties":{"branch":{"type":"string"},"title":{"type":"string"},"body":{"type":"string"}},"required":["branch","title","body"],"additionalProperties":false}"#;

/// The reply shape of the body check, enforced the same way.
const BODY_CHECK_SCHEMA: &str = r#"{"type":"object","properties":{"ok":{"type":"boolean"},"corrections":{"type":"array","items":{"type":"string"}},"body":{"type":["string","null"]}},"required":["ok","corrections","body"],"additionalProperties":false}"#;

/// What the drafter replied: the branch, title and body of the pull request.
/// An empty field falls back to the task's own.
#[derive(Debug, Default, PartialEq, Deserialize)]
struct Draft {
    branch: String,
    title: String,
    body: String,
}

/// What the body check replied. `ok` is the verdict; `body` is the whole
/// corrected body when `ok` is false, and null otherwise.
#[derive(Debug, PartialEq, Deserialize)]
struct BodyCheck {
    ok: bool,
    corrections: Vec<String>,
    body: Option<String>,
}

/// Pauses between the tries to open a pull request: five tries over about
/// half an hour. `ORCHD_PR_RETRY_MS` (comma separated) replaces them in tests.
fn retry_delays() -> Vec<Duration> {
    if let Ok(v) = std::env::var("ORCHD_PR_RETRY_MS") {
        return v
            .split(',')
            .filter_map(|ms| ms.trim().parse().ok())
            .map(Duration::from_millis)
            .collect();
    }
    [60, 240, 600, 900]
        .into_iter()
        .map(Duration::from_secs)
        .collect()
}

/// What a failed attempt means for the retry.
enum PrFailure {
    /// May pass on its own: retried, up to the bound.
    Transient,
    /// Can only repeat: one try and one owner notice. `hint` is what the
    /// owner does about it.
    Permanent(&'static str),
    /// `gh` knows no GitHub host among the repo's remotes: a note, no notice.
    NoGithubHost,
}

/// An orchd refusal is decided by its variant; an outside failure is read by
/// [`classify_external`], the one place that reads git or gh text.
fn failure_of(error: &pr::PrError) -> PrFailure {
    match error {
        pr::PrError::Refused(refusal) if refusal.permanent() => PrFailure::Permanent(""),
        pr::PrError::Refused(_) => PrFailure::Transient,
        pr::PrError::External(text) => classify_external(text),
    }
}

/// Whether a git or gh failure can only repeat (auth, a missing remote, a
/// protected branch, a refused name) or may pass on its own (network, a busy
/// service, a rate limit, a timeout). `gh` exits 1 for every failure, so its
/// message is the only signal. A transient marker wins: a 403 that says "rate
/// limit" is not a refusal.
fn classify_external(error: &str) -> PrFailure {
    const NO_HOST: [&str; 2] = ["known github host", "no github host"];
    const TRANSIENT: [&str; 14] = [
        "rate limit",
        "secondary rate",
        "timed out",
        "temporarily",
        "try again",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
        "connection reset",
        "connection refused",
        "could not resolve",
        "network is unreachable",
        "early eof",
    ];
    const PERMANENT: [&str; 16] = [
        "not logged in",
        "gh auth login",
        "authentication failed",
        "bad credentials",
        "permission denied",
        "permission to",
        "resource not accessible",
        "protected branch",
        "gh006",
        "gh013",
        "does not appear to be a git repository",
        "no such remote",
        "no such file or directory",
        "command not found",
        "repository not found",
        "refusing to allow",
    ];
    let e = error.to_lowercase();
    if NO_HOST.iter().any(|h| e.contains(h)) {
        return PrFailure::NoGithubHost;
    }
    if TRANSIENT.iter().any(|t| e.contains(t)) {
        return PrFailure::Transient;
    }
    let refused =
        PERMANENT.iter().any(|p| e.contains(p)) || e.contains("http 401") || e.contains("http 404");
    if !refused {
        return PrFailure::Transient;
    }
    if e.contains("refusing to allow") {
        PrFailure::Permanent(
            " The token needs the `workflow` scope, or the owner pushes the branch.",
        )
    } else {
        PrFailure::Permanent("")
    }
}

/// Tells the owner in the repo's chat, off the caller's path: the chat can be
/// busy with a live turn, so a refused post is tried again a few times.
pub(super) fn notify_owner(app: &Arc<App>, repo: &str, text: String) {
    let (app, repo) = (app.clone(), repo.to_string());
    tokio::spawn(async move {
        for _ in 0..20 {
            if chat::post_note(&app, &repo, &text) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

/// Marks a task that is about to be saved `done` as owing a pull request:
/// a top-level task whose work did not land on its base, unless the setting
/// is off for its repo. Saved with `done`, so a restart can resume the flow.
pub(super) fn mark_pr_pending(app: &App, task: &mut Task) {
    task.pr_pending = task.parent.is_none()
        && task.pr_url.is_none()
        && task.landed_sha.is_none()
        && !task.archived
        && (task.queue.pr_conflict_tried
            || app.settings.read().unwrap().may_open_pr_on_done(&task.repo));
}

/// Starts the draft and the pull request for a done task that owes one
/// (`pr_pending`): a landing refused on the default branch, or a restart that
/// interrupted the flow.
pub(super) fn spawn_pr_on_done(app: &App, task: &Task) {
    if !task.pr_pending || task.status != TaskStatus::Done || task.parent.is_some() {
        return;
    }
    let id = task.id.clone();
    if task.pr_url.is_some()
        || task.archived
        || !(task.queue.pr_conflict_tried
            || app.settings.read().unwrap().may_open_pr_on_done(&task.repo))
    {
        let _ = app.update_task(&id, |t| t.pr_pending = false);
        return;
    }
    spawn_open_pr(app, &id);
}

/// Runs the flow in the background, without its caller waiting.
pub(super) fn spawn_open_pr(app: &App, id: &str) {
    let (app, id) = (app.arc(), id.to_string());
    tokio::spawn(async move { open_pr_on_done(&app, &id).await });
}

/// Adds decision lines and spend to the task and settles the pending mark
/// when the flow `ends`.
fn record(app: &App, id: &str, lines: Vec<String>, cost: f64, ends: bool) {
    let _ = app.update_task(id, |task| {
        task.decisions.extend(lines);
        task.cost_usd += cost;
        // A task sent back for its resolution keeps its flag and plan.
        if ends && task.status == TaskStatus::Done {
            task.pr_pending = false;
            task.pr_plan = None;
            task.queue.pr_conflict_tried = false;
        }
    });
}

/// Replaces each secret value with `${NAME}` and the home directory with `~`.
/// The body that is published: the unreviewed line and the waived-failures
/// section are put back by code, whatever the drafter or the body check wrote.
fn final_body(
    task: &Task,
    checked: String,
    secrets: &std::collections::HashMap<String, String>,
    home: Option<&str>,
) -> String {
    let source = if task.unreviewed && !checked.starts_with(pr::UNREVIEWED_LINE) {
        format!("{}\n\n{checked}", pr::UNREVIEWED_LINE)
    } else {
        checked
    };
    let waived = redact_pr_text(&pr::waived_section(task), secrets, home);
    let source = pr::without_waived_section(&source);
    truncate_chars(
        &redact_pr_text(&source, secrets, home),
        MAX_BODY_CHARS.saturating_sub(waived.chars().count()),
    ) + &waived
}

fn redact_pr_text(
    text: &str,
    secrets: &std::collections::HashMap<String, String>,
    home: Option<&str>,
) -> String {
    let mut named: Vec<_> = secrets.iter().filter(|(_, v)| !v.is_empty()).collect();
    named.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut out = named
        .into_iter()
        .fold(text.to_string(), |t, (name, value)| {
            t.replace(value.as_str(), &format!("${{{name}}}"))
        });
    if let Some(home) = home
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| !h.is_empty())
    {
        out = out.replace(home, "~");
    }
    out
}

/// The task, while it still owes the automatic pull request: done, pending,
/// not landed, none open, not archived. Landing, a pull request opened by
/// someone else or an archive ends the flow without a word.
fn owed(app: &App, id: &str) -> Option<Task> {
    let task = app.store.load_task(id).ok().flatten()?;
    (task.status == TaskStatus::Done
        && task.pr_pending
        && task.landed_sha.is_none()
        && task.pr_url.is_none()
        && !task.archived)
        .then_some(task)
}

/// Carries the done task's work onto the newest `origin/<base>` before it is
/// pushed, with the task's checks run again when the base moved, as landing
/// does. A branch on an old base would show newer base files (a workflow, a
/// signature) as the task's own change. The branch moves only when the carry
/// and the checks pass; the decisions are saved either way. A carry that
/// conflicts is not a dead end: see [`resolve_conflict`].
pub(super) async fn carry_for_pr(
    app: &Arc<App>,
    task: &Task,
    plan: Option<PrPlan>,
) -> Result<PrCarry, String> {
    let repo = PathBuf::from(&task.repo);
    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || git::default_branch(&repo))
            .await
            .map_err(|e| e.to_string())?
    };
    let base = pr::pr_base(task, &repo, default)
        .await
        .map_err(|e| e.to_string())?;
    pr::run_cmd("git", &["fetch", "origin", &base], &repo, pr::GIT_TIMEOUT).await?;
    let (upstream, local) = (
        format!("origin/{base}"),
        format!("refs/heads/{}", task.branch),
    );
    let contains = pr::run_cmd(
        "git",
        &["merge-base", "--is-ancestor", &upstream, &local],
        &repo,
        pr::GIT_TIMEOUT,
    )
    .await
    .is_ok();
    if contains {
        return Ok(PrCarry::Ready);
    }
    let old = pr::run_cmd("git", &["rev-parse", &local], &repo, pr::GIT_TIMEOUT).await?;
    let path = git::worktree_path(&repo, &app.home, "", &format!("pr-carry-{}", task.id));
    let (r, p, rev) = (repo.clone(), path.clone(), old.clone());
    let temp = tokio::task::spawn_blocking(move || {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| git::GitError(e.to_string()))?;
        }
        git::remove_worktree(&r, &p);
        git::add_detached_worktree(&r, &p, &rev)?;
        Ok::<_, git::GitError>(TempWorktree { repo: r, path: p })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("could not check out {}: {e}", task.branch))?;
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "pr-carry", None)
        .map_err(|e| e.to_string())?;
    // The carry reruns the task's checks here: the tree gets the same
    // environment as a task worktree (the repo's own setup) first.
    {
        let (r, p) = (repo.clone(), temp.path.clone());
        let _ = tokio::task::spawn_blocking(move || git::bootstrap_worktree(&r, &p)).await;
        let sandbox = app.task_sandbox(&task.repo, &temp.path, &run_dir);
        if let Err(e) =
            worktrees::setup_worktree(&repo, &temp.path, sandbox.as_ref(), &CancelToken::new())
                .await
        {
            return Ok(PrCarry::Environment(e));
        }
    }
    let finals = {
        let settings = app.settings.read().unwrap();
        split_deferred(app, &settings, task).0
    };
    let mut work = task.clone();
    let before = work.decisions.len();
    let outcome = carry_and_check(
        app,
        &mut work,
        None,
        &temp.path,
        &run_dir,
        &upstream,
        "Another task",
        &finals,
        &CancelToken::new(),
    )
    .await;
    let conflicted = matches!(
        &outcome,
        Err(Landing::Failed {
            kind: FailureKind::Conflict,
            ..
        })
    );
    let contained = matches!(outcome, Ok(true));
    let committed = match outcome {
        Ok(false) => {
            let (wt, title, tid, n) = (
                temp.path.clone(),
                task.title.clone(),
                task.id.clone(),
                task.attempts.len().max(1) as u32,
            );
            tokio::task::spawn_blocking(move || {
                git::commit(&wt, &title, &tid, n)?;
                git::head_sha(&wt)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("could not commit the carried work: {e}"))
        }
        Ok(true) => Err(format!("{upstream} already contains this work")),
        Err(Landing::Failed { detail, .. }) => Err(detail),
        Err(_) => Err("the carry was interrupted".to_string()),
    };
    let moved = match &committed {
        Ok(sha) => {
            pr::run_cmd(
                "git",
                &["update-ref", &local, sha, &old],
                &repo,
                pr::GIT_TIMEOUT,
            )
            .await
        }
        Err(e) => Err(e.clone()),
    };
    drop(temp);
    // On `contained` the carry's own line is the one decision line.
    let lines = work.decisions[before..].to_vec();
    let ok = moved.is_ok();
    let _ = app.update_task(&task.id, move |t| {
        t.decisions.extend(lines);
        if ok {
            t.base_sha = work.base_sha;
            t.planner_tests = work.planner_tests;
        }
    });
    if contained {
        return Ok(PrCarry::Contained);
    }
    if conflicted {
        return resolve_conflict(app, &task.id, &base, plan).await;
    }
    moved.map(|_| PrCarry::Ready)
}

/// What the carry of a done task onto the newest base came to.
pub(super) enum PrCarry {
    /// The branch holds the newest base; the pull request can be pushed.
    Ready,
    /// The newest base already holds the work: there is nothing to open.
    Contained,
    /// The work conflicts with the newest base and the task went back for its
    /// one resolution attempt; it opens the pull request itself when done.
    Resolving(Vec<String>),
    /// The task ended `failed`, with a report and a notice; the reason.
    Failed(String),
    /// The repo's setup failed in the carry worktree: an environment
    /// problem, not a verdict on the work. Nothing moved.
    Environment(String),
}

/// Waits for the finished loop of `id` to leave `controls`, so that starting
/// the task again is not a no-op.
async fn loop_gone(app: &App, id: &str) -> bool {
    for _ in 0..600 {
        if !app.controls.lock().unwrap().contains_key(id) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The PR carry conflicted: the task gets one conflict-only attempt on the
/// newest base (the loop's own, with its checks and review), and opens the
/// pull request when it is done again. The conflict is carried into the task's
/// own worktree, as the loop carries one onto a moved base. A second conflict,
/// or a carry that cannot be set up, ends the task `failed` with a report and
/// one notice.
async fn resolve_conflict(
    app: &Arc<App>,
    id: &str,
    base: &str,
    plan: Option<PrPlan>,
) -> Result<PrCarry, String> {
    let Some(mut task) = app.store.load_task(id).ok().flatten() else {
        return Err("task not found".to_string());
    };
    let upstream = format!("origin/{base}");
    if task.queue.pr_conflict_tried {
        let reason =
            format!("the work still conflicts with {upstream} after its resolution attempt");
        return Ok(PrCarry::Failed(fail_unresolved(app, id, reason).await));
    }
    // The work conflicts, but only a task started from this very base branch
    // can resolve it in its own worktree; others keep the old outcome.
    if task.base_ref.as_deref() != Some(base) {
        return Err(format!(
            "the work conflicts with {upstream} and the task did not start from {base}, so orchd cannot resolve it"
        ));
    }
    if !loop_gone(app, id).await {
        return Err("the task loop is still running; try again later".to_string());
    }
    let repo = PathBuf::from(&task.repo);
    let tip = pr::run_cmd("git", &["rev-parse", &upstream], &repo, pr::GIT_TIMEOUT).await?;
    task.fresh_base = Some(FreshBase { tip: Some(tip) });
    // Saved before the carry leaves conflict markers in the worktree: a
    // restart in between finds a task that owes a resolution, never a done
    // task with a dirty worktree.
    task.queue.pr_conflict_tried = true;
    // The owner's choices for a manual pull request survive the attempt.
    if plan.is_some() {
        task.pr_pending = true;
        task.pr_plan = plan;
    }
    task.status = TaskStatus::Queued;
    task.updated_at = now_ms();
    app.save_task_logged(&task);
    app.broadcast_task(&task);
    let setup = async {
        let recreated = app.ensure_worktree(&mut task).await?;
        let wt = PathBuf::from(&task.worktree);
        // A recreated worktree has none of its dependencies yet.
        app.ensure_worktree_setup(&task, recreated, &CancelToken::new())
            .await?;
        match carry_onto_newest_base(app, &mut task, &wt, &CancelToken::new()).await {
            BaseCarry::Conflict(_) => Ok(()),
            BaseCarry::Ready => Err(format!("the carry onto {upstream} left no conflict")),
            BaseCarry::Failed(reason) | BaseCarry::Skipped(reason) => Err(reason),
        }
    }
    .await;
    if let Err(e) = setup {
        return Ok(PrCarry::Failed(fail_unresolved(app, id, e).await));
    }
    let files = task.queue.carry_conflicts.clone();
    task.decisions.push(format!(
        "PR: the work conflicts with {upstream} in {}; one conflict-resolution attempt runs on the new base before the pull request opens",
        files.join(", ")
    ));
    task.updated_at = now_ms();
    app.save_task_logged(&task);
    app.broadcast_task(&task);
    app.spawn_task_loop(id.to_string(), true);
    Ok(PrCarry::Resolving(files))
}

/// Ends a done task whose pull request cannot be reached: `failed`, with the
/// reason in its decisions and report, and one notice. The reason is returned.
async fn fail_unresolved(app: &Arc<App>, id: &str, reason: String) -> String {
    let line = format!("PR: not opened: {reason}; the work stays on its branch");
    let _ = app.update_task(id, |t| {
        t.decisions.push(line);
        t.pr_pending = false;
        t.pr_plan = None;
        t.mark_failed();
    });
    if let Ok(Some(mut task)) = app.store.load_task(id) {
        app.write_report(&mut task).await;
        app.save_task_logged(&task);
        app.broadcast_task(&task);
        app.notify_task_failed(&task);
    }
    reason
}

pub(super) async fn open_pr_on_done(app: &Arc<App>, id: &str) {
    let Some(mut task) = owed(app, id) else {
        return record(app, id, Vec::new(), 0.0, true);
    };
    let repo = PathBuf::from(&task.repo);
    if pr::run_cmd(
        "git",
        &["remote", "get-url", "origin"],
        &repo,
        pr::GIT_TIMEOUT,
    )
    .await
    .is_err()
    {
        // Nothing to push to: no draft, no spend, no notice.
        let line = "PR: not opened: the repo has no origin remote".to_string();
        return record(app, id, vec![line], 0.0, true);
    }
    // A remote `gh` does not know (GitLab, a bare path): no draft, no push.
    if let Err(e) = pr::run_cmd(
        &pr::gh_bin(),
        &["repo", "view", "--json", "url"],
        &repo,
        pr::GH_TIMEOUT,
    )
    .await
    {
        if matches!(classify_external(&e), PrFailure::NoGithubHost) {
            return record(app, id, vec![format!("PR: not opened: {e}")], 0.0, true);
        }
    }
    // Never push from a stale base: carry onto the newest one first.
    // A manual `task.pr` carries in the same checkout: one at a time.
    let guard = loop {
        match pr::InFlight::take(id) {
            Ok(guard) => break guard,
            Err(_) if owed(app, id).is_some() => {
                tokio::time::sleep(Duration::from_millis(200)).await
            }
            Err(_) => return record(app, id, Vec::new(), 0.0, true),
        }
    };
    let carried = carry_for_pr(app, &task, None).await;
    drop(guard);
    match carried {
        Ok(PrCarry::Ready) => {}
        // One decision line was saved; there is no pull request to open.
        Ok(PrCarry::Contained) => return record(app, id, Vec::new(), 0.0, true),
        // The task is back in its loop and opens the pull request when done.
        Ok(PrCarry::Resolving(_) | PrCarry::Failed(_)) => return,
        // setup_failed: the environment, not the work. The branch is
        // untouched; the owner hears once and opens it with task_pr.
        Ok(PrCarry::Environment(e)) => {
            record(
                app,
                id,
                vec![format!(
                    "PR: not opened: the setup of the carry worktree failed ({e}); this is the environment, the work is untouched and nothing was pushed"
                )],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened: the repo's worktree setup failed while carrying onto the newest base ({e}). The work is on branch {}. Fix the setup, then open it with task_pr.",
                    task.title, task.branch
                ),
            );
            return;
        }
        Err(e) => {
            record(
                app,
                id,
                vec![format!(
                    "PR: not opened: the work could not be carried onto the newest base ({e}); nothing was pushed"
                )],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened: the work could not be carried onto the newest base ({e}). Nothing was pushed; the work is on branch {}. Carry it by hand, then open it with task_pr.",
                    task.title, task.branch
                ),
            );
            return;
        }
    }
    let Some(fresh) = owed(app, id) else {
        return record(app, id, Vec::new(), 0.0, true);
    };
    task = fresh;
    // A restart reuses what the first run chose and drafts nothing again.
    let plan = match task.pr_plan.clone() {
        Some(plan) => plan,
        None => {
            let mut cost = 0.0;
            let mut drafted_ok = true;
            let (mut lines, draft) = match draft_pr(app, &task, &mut cost).await {
                Ok((route, draft)) => (vec![format!("PR: drafted by {route}")], draft),
                Err(reason) => {
                    drafted_ok = false;
                    (
                        vec![format!(
                            "PR: draft failed ({reason}); opened with the task's own title and body"
                        )],
                        Draft::default(),
                    )
                }
            };
            let branch = match choose_branch(&task, &draft.branch).await {
                Ok(name) => name,
                Err(why) => {
                    if drafted_ok {
                        lines.push(format!(
                            "PR: branch: {why}; the task's own branch {} is used",
                            task.branch
                        ));
                    }
                    task.branch.clone()
                }
            };
            // Landed, or opened by the overseer or the owner, while the draft ran.
            let Some(task) = owed(app, id) else {
                if let Ok(Some(now)) = app.store.load_task(id) {
                    if let Some(url) = now.pr_url {
                        lines.push(format!("PR: already open {url}, draft not used"));
                    }
                }
                return record(app, id, lines, cost, true);
            };
            let secrets = match run_secrets(app, id) {
                Ok(secrets) => secrets.redaction,
                Err(e) => {
                    lines.push(format!("PR: not opened: {e}"));
                    return record(app, id, lines, cost, true);
                }
            };
            let home = std::env::var("HOME").ok();
            let title = redact_pr_text(&draft.title, &secrets, home.as_deref());
            // The task's own body when the draft has none: it is published too.
            let drafted = if draft.body.trim().is_empty() {
                pr::pr_body(&task)
            } else {
                draft.body.clone()
            };
            // One agent turn checks the body against the final diff and the
            // check results before anything is pushed. A failed turn keeps the
            // drafted body; the owner reads the line either way.
            let checked = match check_body(app, &task, &drafted, &mut cost).await {
                Ok(check) if check.ok => drafted,
                Ok(check) => {
                    lines.push(format!("PR: body check: {}", check.corrections.join("; ")));
                    check
                        .body
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or(drafted)
                }
                Err(reason) => {
                    lines.push(format!(
                        "PR: body check failed ({reason}); opened as drafted"
                    ));
                    drafted
                }
            };
            let body = final_body(&task, checked, &secrets, home.as_deref());
            let plan = PrPlan {
                branch,
                title,
                body,
            };
            let saved = plan.clone();
            let _ = app.update_task(id, move |t| {
                t.decisions.extend(lines);
                t.cost_usd += cost;
                t.pr_plan = Some(saved);
            });
            plan
        }
    };
    let (mut tries, delays) = (0, retry_delays());
    loop {
        if owed(app, id).is_none() {
            return record(app, id, Vec::new(), 0.0, true);
        }
        let params = pr::PrParams {
            id: id.to_string(),
            title: Some(plan.title.clone()),
            branch: Some(plan.branch.clone()),
            body: Some(plan.body.clone()),
            source: None,
        };
        let Err(e) = app.open_pr(params, true).await else {
            return record(app, id, Vec::new(), 0.0, true);
        };
        // Opened or landed meanwhile: nothing to report.
        if owed(app, id).is_none() {
            return record(app, id, Vec::new(), 0.0, true);
        }
        let (permanent, hint) = match failure_of(&e) {
            PrFailure::NoGithubHost => {
                return record(app, id, vec![format!("PR: not opened: {e}")], 0.0, true);
            }
            PrFailure::Permanent(hint) => (true, hint),
            PrFailure::Transient => (false, ""),
        };
        if permanent || tries >= delays.len() {
            let tried = if permanent {
                String::new()
            } else {
                format!(" after {} tries", tries + 1)
            };
            record(
                app,
                id,
                vec![format!("PR: not opened{tried}: {e}.{hint}")],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened{tried}: {e}.{hint} The work is on branch {}. Fix that, then open it with task_pr.",
                    task.title, task.branch
                ),
            );
            return;
        }
        let wait = delays[tries];
        tries += 1;
        let line = format!(
            "PR: try {tries} failed ({e}); trying again in {}s",
            wait.as_secs()
        );
        record(app, id, vec![line], 0.0, false);
        tokio::time::sleep(wait).await;
        // A shutdown keeps `pr_pending`: the next start resumes the flow.
        if app.shutting_down.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// A temporary detached worktree of a branch, removed when dropped.
struct TempWorktree {
    repo: PathBuf,
    path: PathBuf,
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        git::remove_worktree(&self.repo, &self.path);
    }
}

/// The cost stage both PR helper turns book under (`costs::STAGES`); the body
/// check is part of drafting the pull request.
const STAGE: &str = "pr_draft";

/// What one read-only helper turn runs with.
struct HelperRun<'a> {
    /// The turn's run directory, guard and log name.
    name: &'static str,
    /// The reply shape the harness enforces for this turn.
    schema: &'static str,
    route: &'a Route,
    settings: &'a Settings,
    worktree: &'a Path,
    brief: &'a str,
}

/// Runs a helper turn in the task's worktree, else in a temporary detached
/// worktree of its branch. `make_brief` gets the diff against the base.
/// Returns the route id and the run's outcome; `cost` gets the spend.
async fn helper_turn(
    app: &Arc<App>,
    task: &Task,
    name: &'static str,
    schema: &'static str,
    make_brief: impl FnOnce(&str) -> String,
    cost: &mut f64,
) -> Result<(String, harness::RunOutcome), String> {
    let settings = app.settings.read().unwrap().clone();
    let route = settings
        .pr_draft_route()
        .cloned()
        .ok_or("no route to draft with")?;
    let (route, _) = app
        .usable_route(&settings, route)
        .ok_or("no agent harness is available")?;
    let repo = PathBuf::from(&task.repo);
    let temp = if Path::new(&task.worktree).join(".git").exists() {
        None
    } else {
        let path = git::worktree_path(&repo, &app.home, "", &format!("pr-draft-{}", task.id));
        let (r, p, rev) = (
            repo.clone(),
            path.clone(),
            format!("refs/heads/{}", task.branch),
        );
        tokio::task::spawn_blocking(move || {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| git::GitError(e.to_string()))?;
            }
            git::remove_worktree(&r, &p);
            git::add_detached_worktree(&r, &p, &rev)?;
            Ok::<_, git::GitError>(TempWorktree { repo: r, path: p })
        })
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
        .map_err(|e| format!("could not check out {}: {e}", task.branch))?
    };
    let worktree = temp
        .as_ref()
        .map_or_else(|| PathBuf::from(&task.worktree), |t| t.path.clone());
    let (wt, base) = (worktree.clone(), task.base_sha.clone());
    let diff = tokio::task::spawn_blocking(move || {
        git::diff_full(&wt, &base, MAX_DIFF_CHARS).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let brief = make_brief(&diff);
    let run = HelperRun {
        name,
        schema,
        route: &route,
        settings: &settings,
        worktree: &worktree,
        brief: &brief,
    };
    let outcome = run_helper(app, task, run, cost).await;
    drop(temp);
    outcome.map(|outcome| (route.id, outcome))
}

/// Gates the branch name the drafter chose: a valid ref name that is not the
/// default or base branch, and not a branch that already exists on the push
/// remote or locally (the task's own excepted). `Err` is the reason the
/// task's own branch is used instead.
async fn choose_branch(task: &Task, drafted: &str) -> Result<String, String> {
    // A pull request already open keeps its head.
    if let Some(open) = task.pr_branch.as_deref().filter(|_| task.pr_url.is_some()) {
        return Ok(open.to_string());
    }
    let name = drafted.trim();
    if name.is_empty() {
        return Err("the draft named none".to_string());
    }
    let bad = |why: &str| format!("the drafted name {name} {why}");
    if pr::check_branch_syntax(name).is_err() {
        return Err(bad("is not a usable branch name"));
    }
    if name == task.branch {
        return Ok(task.branch.clone());
    }
    let repo = PathBuf::from(&task.repo);
    async fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
        pr::run_cmd("git", args, repo, pr::GIT_TIMEOUT).await
    }
    if git(&repo, &["check-ref-format", "--branch", name])
        .await
        .is_err()
    {
        return Err(bad("is not a valid branch name"));
    }
    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || git::default_branch(&repo))
            .await
            .map_err(|e| e.to_string())?
    };
    if [default.as_deref(), task.base_ref.as_deref()].contains(&Some(name)) {
        return Err(bad("is the base branch"));
    }
    let full = format!("refs/heads/{name}");
    if git(&repo, &["show-ref", "--verify", "-q", &full])
        .await
        .is_ok()
    {
        return Err(bad("is a local branch of another task"));
    }
    match git(&repo, &["ls-remote", "--heads", "origin", &full]).await {
        Ok(out) if out.trim().is_empty() => Ok(name.to_string()),
        Ok(_) => Err(bad("exists on the remote")),
        Err(e) => Err(format!("the remote could not be checked ({e})")),
    }
}

/// Drafts the branch, title and body of the pull request; `cost` gets the spend.
async fn draft_pr(app: &Arc<App>, task: &Task, cost: &mut f64) -> Result<(String, Draft), String> {
    let (route, outcome) = helper_turn(
        app,
        task,
        "pr-draft",
        DRAFT_SCHEMA,
        |diff| brief::build_pr_draft_brief(task, diff),
        cost,
    )
    .await?;
    let draft = schema_reply(&outcome, "draft")?;
    Ok((route, draft))
}

/// Checks the drafted body against the final diff and the check results; the
/// reply is the verdict. `cost` gets the spend.
async fn check_body(
    app: &Arc<App>,
    task: &Task,
    body: &str,
    cost: &mut f64,
) -> Result<BodyCheck, String> {
    let (_, outcome) = helper_turn(
        app,
        task,
        "pr-truth",
        BODY_CHECK_SCHEMA,
        |diff| brief::build_pr_truth_brief(task, body, diff),
        cost,
    )
    .await?;
    schema_reply(&outcome, "body check")
}

/// The turn's reply, read as the schema the harness enforced. A reply that
/// does not deserialize fails the turn; it is never read leniently.
fn schema_reply<T: serde::de::DeserializeOwned>(
    outcome: &harness::RunOutcome,
    what: &str,
) -> Result<T, String> {
    if outcome.timed_out {
        return Err("the run timed out".to_string());
    }
    if let Some(error) = &outcome.error {
        return Err(error.clone());
    }
    serde_json::from_str(outcome.final_text.as_deref().unwrap_or_default())
        .map_err(|_| format!("the reply did not match the {what} schema"))
}

/// One read-only agent turn with the brief, in the run's worktree, under the
/// turn's schema. `cost` gets the spend of a run that finished.
async fn run_helper(
    app: &Arc<App>,
    task: &Task,
    run: HelperRun<'_>,
    cost: &mut f64,
) -> Result<harness::RunOutcome, String> {
    let HelperRun {
        name,
        schema,
        route,
        settings,
        worktree,
        brief,
    } = run;
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, name, None)
        .map_err(|e| e.to_string())?;
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), brief);
    let events_path = run_dir.join("events.jsonl");
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            route,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        max_budget_usd: None,
        review: true,
        task_run: false,
        task_sandbox: None,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: &[],
        images: &[],
        repo_settings: true,
        resume: None,
    };
    let schema_path = run_dir.join("schema.json");
    let _ = std::fs::write(&schema_path, schema);
    let output = harness::OutputSchema {
        json: schema,
        path: &schema_path,
    };
    let result = run_harness_schema(
        app,
        &task.id,
        0,
        RunTrack::No,
        worktree,
        &req,
        CostTag::task(STAGE, &route.id),
        brief,
        &events_path,
        &CancelToken::new(),
        Guard::helper(name, app.helper_timeout()),
        Some(&output),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => return Err("the run was stopped".to_string()),
        Err(RunError::Io(e) | RunError::NotFound(e)) => return Err(e),
    };
    *cost += outcome.cost_usd.unwrap_or(0.0);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::task_with_status;

    #[test]
    fn the_waived_section_survives_a_drafted_and_a_rewritten_body() {
        let mut task = task_with_status(TaskStatus::Done);
        task.assumptions.push(Assumption {
            question: "q".into(),
            answer: WAIVED.into(),
            evidence: "alpha fails on the base".into(),
            by: WAIVER_BY.into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: Some(1),
            commands: vec!["check".into()],
            overturned: false,
            owner_answer: None,
        });
        let none = std::collections::HashMap::new();
        for drafted in [
            "Why this change\n",
            "Why\n\n## Waived base failures\n\n- invented\n",
        ] {
            let body = final_body(&task, drafted.to_string(), &none, None);
            assert_eq!(body.matches("## Waived base failures").count(), 1, "{body}");
            assert!(body.contains("- check: alpha fails on the base"), "{body}");
            assert!(!body.contains("invented"), "{body}");
        }
        task.unreviewed = true;
        let body = final_body(&task, "Why\n".into(), &none, None);
        assert!(body.starts_with(pr::UNREVIEWED_LINE) && body.contains("alpha fails"));
    }

    #[test]
    fn a_schema_checked_reply_is_read_and_anything_else_fails_the_turn() {
        let json = r#"{"branch":"feature/x","title":"feat: x","body":"Why\n"}"#;
        let want = Draft {
            branch: "feature/x".into(),
            title: "feat: x".into(),
            body: "Why\n".into(),
        };
        let reply = |text: &str| {
            let stream = format!(
                "{}\n",
                serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "result": text})
            );
            harness::replay_events(Harness::Claude, &stream)
        };
        assert_eq!(schema_reply(&reply(json), "draft"), Ok(want));
        for bad in [
            format!("```json\n{json}\n```"),
            format!("Here you go: {json} done"),
            r#"{"title":"t","body":""}"#.to_string(),
            r#"{"branch":"b","title":5,"body":""}"#.to_string(),
            "no json here".to_string(),
        ] {
            assert_eq!(
                schema_reply::<Draft>(&reply(&bad), "draft"),
                Err("the reply did not match the draft schema".to_string()),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_body_check_needs_an_explicit_verdict() {
        let fixed: BodyCheck = serde_json::from_str(
            r#"{"ok":false,"corrections":["the diff adds no tests"],"body":"Fixed"}"#,
        )
        .unwrap();
        assert!(!fixed.ok);
        assert_eq!(
            fixed.corrections,
            vec!["the diff adds no tests".to_string()]
        );
        assert_eq!(fixed.body.as_deref(), Some("Fixed"));
        let clean: BodyCheck =
            serde_json::from_str(r#"{"ok":true,"corrections":[],"body":null}"#).unwrap();
        assert!(clean.ok && clean.body.is_none());
        assert!(serde_json::from_str::<BodyCheck>(r#"{"corrections":[],"body":null}"#).is_err());
    }

    #[test]
    fn the_draft_turn_passes_its_schema_to_each_harness() {
        let wt = Path::new("/worktree");
        let schema_path = Path::new("/run/schema.json");
        let schema = harness::OutputSchema {
            json: DRAFT_SCHEMA,
            path: schema_path,
        };
        let req = |harness| harness::RunRequest {
            harness,
            worktree: wt,
            model: None,
            effort: None,
            max_budget_usd: None,
            review: true,
            task_run: false,
            task_sandbox: None,
            mcp_config: None,
            settings_path: None,
            network_allowed: false,
            codex_mcp: &[],
            images: &[],
            repo_settings: true,
            resume: None,
        };
        let claude = harness::build_argv_with_schema(&req(Harness::Claude), Some(&schema));
        let flag = claude.iter().position(|a| a == "--json-schema").unwrap();
        assert_eq!(claude[flag + 1], DRAFT_SCHEMA);
        let codex = harness::build_argv_with_schema(&req(Harness::Codex), Some(&schema));
        let flag = codex.iter().position(|a| a == "--output-schema").unwrap();
        assert_eq!(codex[flag + 1], "/run/schema.json");
        assert_eq!(codex.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn redaction_names_secrets_and_hides_the_home_directory() {
        let secrets = std::collections::HashMap::from([
            ("API_KEY".to_string(), "s3cr3t-value".to_string()),
            ("SHORT".to_string(), "s3cr3t".to_string()),
            ("EMPTY".to_string(), String::new()),
        ]);
        let out = redact_pr_text(
            "key s3cr3t-value and s3cr3t at /home/zed/proj/a.rs",
            &secrets,
            Some("/home/zed/"),
        );
        assert_eq!(out, "key ${API_KEY} and ${SHORT} at ~/proj/a.rs");
        assert_eq!(redact_pr_text("/x", &secrets, Some("/")), "/x");
        assert_eq!(redact_pr_text("plain", &secrets, None), "plain");
    }

    #[test]
    fn a_rate_limit_or_a_timeout_is_transient_even_with_a_403() {
        for e in [
            "gh pr: HTTP 403: API rate limit exceeded",
            "gh pr failed: HTTP 403: You have exceeded a secondary rate limit",
            "git ls-remote timed out after 60s",
            "gh pr failed: HTTP 502: Bad Gateway, try again later",
            "git push failed: Could not resolve host: example.invalid",
            "gh pr failed: connection reset by peer",
        ] {
            assert!(matches!(classify_external(e), PrFailure::Transient), "{e}");
        }
        for e in [
            "gh pr failed: To get started, please run:  gh auth login",
            "git push failed: Permission denied (publickey)",
            "git push failed: remote: error: GH006: Protected branch update failed",
            "gh pr failed: HTTP 401: Bad credentials",
            "git fetch failed: fatal: 'origin' does not appear to be a git repository",
        ] {
            assert!(
                matches!(classify_external(e), PrFailure::Permanent(_)),
                "{e}"
            );
        }
        let scope = "remote: refusing to allow a Personal Access Token to create or update workflow `.github/workflows/ci.yml` without `workflow` scope";
        let PrFailure::Permanent(hint) = classify_external(scope) else {
            panic!("a scope refusal is permanent");
        };
        assert!(hint.contains("workflow"), "{hint}");
        assert!(matches!(
            classify_external("none of the git remotes point to a known GitHub host"),
            PrFailure::NoGithubHost
        ));
    }

    #[test]
    fn an_orchd_refusal_is_decided_by_its_variant_not_its_text() {
        let carry = pr::PrError::Refused(pr::PrRefusal::CarryFailed(
            "could not reach origin; try again later".into(),
        ));
        assert!(matches!(failure_of(&carry), PrFailure::Permanent(_)));
        let busy = pr::PrError::Refused(pr::PrRefusal::InFlight);
        assert!(matches!(failure_of(&busy), PrFailure::Transient));
        let push = pr::PrError::External("git push failed: connection reset".into());
        assert!(matches!(failure_of(&push), PrFailure::Transient));
    }
}
