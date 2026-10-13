//! A done top-level task that could not land on the default branch gets a pull
//! request. One read-only helper drafts the branch name, title and body by the
//! repo's own rules; orchd then opens it through `task.pr`. The run holds no
//! slot, never blocks the task loop and never changes the task's status.

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

/// Marks a task that is about to be saved `done` as owing a pull request,
/// when it is a top-level task and its repo opted in. Saved with `done`, so
/// a restart can resume the flow.
pub(super) fn mark_pr_pending(app: &App, task: &mut Task) {
    task.pr_pending = task.parent.is_none()
        && task.pr_url.is_none()
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

pub(super) async fn open_pr_on_done(app: &Arc<App>, id: &str) {
    let Ok(Some(task)) = app.store.load_task(id) else {
        return;
    };
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
    // The overseer may have opened the pull request while the draft ran.
    if let Ok(Some(now)) = app.store.load_task(id) {
        if let Some(url) = now.pr_url {
            lines.push(format!("PR: already open {url}, draft not used"));
            record(app, id, lines, cost, true);
            return;
        }
    }
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
    record(app, id, lines, cost, false);
    let secrets = match run_secrets(app, id) {
        Ok(secrets) => secrets.redaction,
        Err(e) => {
            record(app, id, vec![format!("PR: not opened: {e}")], 0.0, true);
            return;
        }
    };
    let home = std::env::var("HOME").ok();
    let title = redact_pr_text(&draft.title, &secrets, home.as_deref());
    let body = truncate_chars(
        &redact_pr_text(&draft.body, &secrets, home.as_deref()),
        MAX_BODY_CHARS,
    );
    let open = |branch: &str| {
        app.handle_task_pr(serde_json::json!({
            "id": id, "branch": branch, "title": title, "body": body,
        }))
    };
    let mut result = open(&branch).await;
    if let Err(e) = &result {
        if branch != task.branch && e.contains("branch") {
            record(
                app,
                id,
                vec![format!(
                    "PR: drafted branch {branch} refused ({e}); using {}",
                    task.branch
                )],
                0.0,
                false,
            );
            result = open(&task.branch).await;
        }
    }
    let end = match result {
        Ok(_) => Vec::new(),
        Err(e) => vec![format!("PR: not opened: {e}")],
    };
    record(app, id, end, 0.0, true);
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
    fn rejects_text_without_a_json_object_and_defaults_missing_fields() {
        assert_eq!(parse_draft("no json here"), None);
        assert_eq!(parse_draft("} backwards {"), None);
        assert_eq!(parse_draft(r#"{"title":5}"#), None);
        let draft = parse_draft(r#"{"title":"t"}"#).unwrap();
        assert_eq!((draft.title.as_str(), draft.branch.as_str()), ("t", ""));
    }
}
