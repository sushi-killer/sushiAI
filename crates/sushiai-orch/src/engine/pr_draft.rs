//! A done top-level task whose work did not land on its base gets a pull
//! request, with no help from the overseer. One read-only helper drafts the
//! branch name, title and body by the repo's own rules; orchd then opens it
//! through `task.pr`. A transient failure (network, `gh`) is retried with a
//! growing delay; an auth or permanent failure sends the owner one notice. The
//! run holds no slot, never blocks the task loop and never changes the task's
//! status: it stays done, and `task.pr` can open the pull request later.

use super::*;

/// Longest diff given to the drafter, in characters.
const MAX_DIFF_CHARS: usize = 20_000;
/// Longest pull request body sent to GitHub, in characters.
const MAX_BODY_CHARS: usize = 20_000;

/// What the drafter replied; an empty field falls back to the task's own.
#[derive(Debug, Default, PartialEq, Deserialize)]
struct Draft {
    #[serde(default)]
    branch: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

/// The JSON object in a reply, with or without a code fence or prose around it.
fn parse_draft(text: &str) -> Option<Draft> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
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

/// Whether a failed attempt can only repeat (auth, a missing remote, a
/// protected branch, a refused name) or may pass on its own (network, a busy
/// service, a rate limit, a timeout). A transient marker wins: a 403 that says
/// "rate limit" is not a refusal.
fn is_permanent(error: &str) -> bool {
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
    const PERMANENT: [&str; 24] = [
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
        "no default branch",
        "no commits beyond",
        "already open",
        "not a valid branch name",
        "no longer exists",
        "not part of this task",
        "is a base branch",
        "already landed",
        "is archived",
        "repository not found",
    ];
    let e = error.to_lowercase();
    if TRANSIENT.iter().any(|t| e.contains(t)) {
        return false;
    }
    PERMANENT.iter().any(|p| e.contains(p)) || e.contains("http 401") || e.contains("http 404")
}

/// `gh` found no GitHub host among the repo's remotes (GitLab, a bare path).
fn is_no_github_host(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("known github host") || e.contains("no github host")
}

/// Repos that already got the "not a GitHub remote" notice in this run.
static NO_HOST_NOTICED: StdMutex<Option<std::collections::HashSet<String>>> = StdMutex::new(None);

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
        && app.settings.read().unwrap().may_open_pr_on_done(&task.repo);
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
        || !app.settings.read().unwrap().may_open_pr_on_done(&task.repo)
    {
        let _ = app.update_task(&id, |t| t.pr_pending = false);
        return;
    }
    let app = app.arc();
    tokio::spawn(async move { open_pr_on_done(&app, &id).await });
}

/// Adds decision lines and spend to the task and settles the pending mark
/// when the flow `ends`.
fn record(app: &App, id: &str, lines: Vec<String>, cost: f64, ends: bool) {
    let _ = app.update_task(id, |task| {
        task.decisions.extend(lines);
        task.cost_usd += cost;
        if ends {
            task.pr_pending = false;
            task.pr_plan = None;
        }
    });
}

/// Replaces each secret value with `${NAME}` and the home directory with `~`.
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

pub(super) async fn open_pr_on_done(app: &Arc<App>, id: &str) {
    let Some(task) = owed(app, id) else {
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
    // A restart reuses what the first run chose and drafts nothing again.
    let plan = match task.pr_plan.clone() {
        Some(plan) => plan,
        None => {
            let mut cost = 0.0;
            let (mut lines, draft) = match draft_pr(app, &task, &mut cost).await {
                Ok((route, draft)) => (vec![format!("PR: drafted by {route}")], draft),
                Err(reason) => (
                    vec![format!(
                        "PR: draft failed ({reason}); opened with the task's own title and branch"
                    )],
                    Draft::default(),
                ),
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
            let mut branch = task.branch.clone();
            let drafted = draft.branch.trim();
            if !drafted.is_empty() && drafted != task.branch {
                match pr::check_draft_branch(&task, drafted).await {
                    Ok(()) => branch = drafted.to_string(),
                    Err(why) => lines.push(format!(
                        "PR: drafted branch {drafted} refused ({why}); using {}",
                        task.branch
                    )),
                }
            }
            let secrets = match run_secrets(app, id) {
                Ok(secrets) => secrets.redaction,
                Err(e) => {
                    let line = format!("PR: not opened: {e}");
                    lines.push(line);
                    return record(app, id, lines, cost, true);
                }
            };
            let home = std::env::var("HOME").ok();
            let title = redact_pr_text(&draft.title, &secrets, home.as_deref());
            // The task's own body when the draft has none: it is published too.
            let source = if draft.body.trim().is_empty() {
                super::pr::pr_body(&task)
            } else {
                draft.body.clone()
            };
            let body = truncate_chars(
                &redact_pr_text(&source, &secrets, home.as_deref()),
                MAX_BODY_CHARS,
            );
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
        let result = app
            .handle_task_pr(serde_json::json!({
                "id": id, "branch": plan.branch, "title": plan.title, "body": plan.body,
            }))
            .await;
        let Err(e) = result else {
            return record(app, id, Vec::new(), 0.0, true);
        };
        // Opened or landed meanwhile: nothing to report.
        if owed(app, id).is_none() {
            return record(app, id, Vec::new(), 0.0, true);
        }
        if is_no_github_host(&e) {
            record(app, id, vec![format!("PR: not opened: {e}")], 0.0, true);
            let first = NO_HOST_NOTICED
                .lock()
                .unwrap()
                .get_or_insert_with(Default::default)
                .insert(task.repo.clone());
            if first {
                notify_owner(
                    app,
                    &task.repo,
                    format!(
                        "Pull requests are not opened for this repo: its remote is not a GitHub host ({e}). The finished work stays on each task's own branch."
                    ),
                );
            }
            return;
        }
        let permanent = is_permanent(&e);
        if permanent || tries >= delays.len() {
            let tried = if permanent {
                String::new()
            } else {
                format!(" after {} tries", tries + 1)
            };
            record(
                app,
                id,
                vec![format!("PR: not opened{tried}: {e}")],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened{tried}: {e}. The work is on branch {}. Fix that, then open it with task_pr.",
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

/// Runs the helper in the task's worktree, else in a temporary detached
/// worktree of its branch. Returns the route id and the parsed draft; `cost`
/// gets the run's spend.
async fn draft_pr(app: &Arc<App>, task: &Task, cost: &mut f64) -> Result<(String, Draft), String> {
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
    let draft = run_draft(app, task, &settings, &route, &worktree, cost).await;
    drop(temp);
    draft.map(|draft| (route.id, draft))
}

async fn run_draft(
    app: &Arc<App>,
    task: &Task,
    settings: &Settings,
    route: &Route,
    worktree: &Path,
    cost: &mut f64,
) -> Result<Draft, String> {
    let (wt, base) = (worktree.to_path_buf(), task.base_sha.clone());
    let diff = tokio::task::spawn_blocking(move || {
        git::diff_full(&wt, &base, MAX_DIFF_CHARS).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let brief_text = brief::build_pr_draft_brief(task, &diff);

    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "pr-draft", None)
        .map_err(|e| e.to_string())?;
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
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
    let result = run_harness(
        app,
        &task.id,
        0,
        RunTrack::No,
        worktree,
        &req,
        CostTag::task("pr_draft", &route.id),
        &brief_text,
        &events_path,
        &CancelToken::new(),
        Guard::helper("pr-draft", app.helper_timeout()),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => return Err("the run was stopped".to_string()),
        Err(RunError::Io(e) | RunError::NotFound(e)) => return Err(e),
    };
    *cost = outcome.cost_usd.unwrap_or(0.0);
    if outcome.timed_out {
        return Err("the run timed out".to_string());
    }
    if let Some(error) = outcome.error {
        return Err(error);
    }
    parse_draft(&outcome.final_text.unwrap_or_default())
        .ok_or_else(|| "the reply was not a JSON object".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_fenced_and_surrounded_json() {
        let want = Draft {
            branch: "feat/x".into(),
            title: "feat: x".into(),
            body: "Why\n".into(),
        };
        let json = r#"{"branch":"feat/x","title":"feat: x","body":"Why\n"}"#;
        assert_eq!(parse_draft(json), Some(want));
        let fenced = format!("```json\n{json}\n```");
        assert!(parse_draft(&fenced).is_some());
        assert!(parse_draft(&format!("Here you go: {json} done")).is_some());
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
        ] {
            assert!(!is_permanent(e), "{e}");
        }
        for e in [
            "gh pr failed: To get started, please run:  gh auth login",
            "git push failed: Permission denied (publickey)",
            "git push failed: remote: error: GH006: Protected branch update failed",
            "gh pr failed: HTTP 401: Bad credentials",
            "git fetch failed: fatal: 'origin' does not appear to be a git repository",
        ] {
            assert!(is_permanent(e), "{e}");
        }
    }

    #[test]
    fn rejects_text_without_a_json_object_and_defaults_missing_fields() {
        assert_eq!(parse_draft("no json here"), None);
        assert_eq!(parse_draft("} backwards {"), None);
        assert_eq!(parse_draft(r#"{"title":5}"#), None);
        let draft = parse_draft(r#"{"title":"t"}"#).unwrap();
        assert_eq!((draft.title.as_str(), draft.branch.as_str()), ("t", ""));
    }
}
