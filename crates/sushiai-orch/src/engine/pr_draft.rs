//! A done top-level task that could not land on the default branch gets a pull
//! request. One read-only helper drafts the branch name, title and body by the
//! repo's own rules; orchd then opens it through `task.pr`. The run holds no
//! slot, never blocks the task loop and never changes the task's status.

use super::*;

/// Longest diff given to the drafter, in characters.
const MAX_DIFF_CHARS: usize = 20_000;

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

/// Starts the draft and the pull request for a finished task whose landing
/// was refused because its base is the default branch, when the repo opted in.
pub(super) fn spawn_pr_on_done(app: &Arc<App>, task: &Task) {
    if task.parent.is_some()
        || task.pr_url.is_some()
        || task.archived
        || !app.settings.read().unwrap().may_open_pr_on_done(&task.repo)
    {
        return;
    }
    let (app, id) = (app.clone(), task.id.clone());
    tokio::spawn(async move { open_pr_on_done(&app, &id).await });
}

fn note(app: &App, id: &str, line: String) {
    if let Ok(Some(mut task)) = app.store.load_task(id) {
        task.decisions.push(line);
        task.updated_at = now_ms();
        app.save_task_logged(&task);
        app.broadcast_task(&task);
    }
}

pub(super) async fn open_pr_on_done(app: &Arc<App>, id: &str) {
    let Ok(Some(task)) = app.store.load_task(id) else {
        return;
    };
    let draft = match draft_pr(app, &task).await {
        Ok((route, draft)) => {
            note(app, id, format!("PR: drafted by {route}"));
            draft
        }
        Err(reason) => {
            note(
                app,
                id,
                format!("PR: draft failed ({reason}); opened with the task's own title and branch"),
            );
            Draft::default()
        }
    };
    let params = serde_json::json!({
        "id": id,
        "branch": draft.branch,
        "title": draft.title,
        "body": draft.body,
    });
    if let Err(e) = app.handle_task_pr(params).await {
        note(app, id, format!("PR: not opened: {e}"));
    }
}

/// Runs the helper in the task's worktree, else in a temporary detached
/// worktree of its branch. Returns the route id and the parsed draft.
async fn draft_pr(app: &Arc<App>, task: &Task) -> Result<(String, Draft), String> {
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
            git::add_detached_worktree(&r, &p, &rev)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("could not check out {}: {e}", task.branch))?;
        Some(path)
    };
    let worktree = temp
        .clone()
        .unwrap_or_else(|| PathBuf::from(&task.worktree));
    let result = run_draft(app, task, &settings, &route, &worktree).await;
    if let Some(path) = temp {
        let _ = tokio::task::spawn_blocking(move || git::remove_worktree(&repo, &path)).await;
    }
    result.map(|draft| (route.id, draft))
}

async fn run_draft(
    app: &Arc<App>,
    task: &Task,
    settings: &Settings,
    route: &Route,
    worktree: &Path,
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
    fn rejects_text_without_a_json_object_and_defaults_missing_fields() {
        assert_eq!(parse_draft("no json here"), None);
        assert_eq!(parse_draft("} backwards {"), None);
        assert_eq!(parse_draft(r#"{"title":5}"#), None);
        let draft = parse_draft(r#"{"title":"t"}"#).unwrap();
        assert_eq!((draft.title.as_str(), draft.branch.as_str()), ("t", ""));
    }
}
