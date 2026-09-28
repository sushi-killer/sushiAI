use super::*;

/// Simple glob match: `*` matches any run of characters (including `/`,
/// deliberately simpler than shell globbing since protected paths are
/// meant to be broad, e.g. `src/app/**`), `?` matches one character,
/// anything else matches literally.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_chars(&p, &t)
}

fn glob_match_chars(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            if glob_match_chars(&p[1..], t) {
                return true;
            }
            !t.is_empty() && glob_match_chars(p, &t[1..])
        }
        '?' => !t.is_empty() && glob_match_chars(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && glob_match_chars(&p[1..], &t[1..]),
    }
}

pub fn matches_any_protected(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| glob_match(g, path))
}

/// `review == "auto"` -> the hard tier's route, so a cheaper implementer is
/// checked by the strongest model; when the implementer already *is* that
/// route, the first route on a different harness instead. Explicit id ->
/// that route; `""` -> no review (handled by the caller before this).
pub fn select_review_route<'a>(
    settings: &'a Settings,
    implementer: &Route,
    other_family: bool,
) -> Option<&'a Route> {
    if settings.review != "auto" {
        return settings.routes.iter().find(|r| r.id == settings.review);
    }
    let hard = settings.tiers.get(&Tier::Hard);
    if other_family {
        let other = |r: &&Route| r.harness != implementer.harness;
        let found = settings
            .routes
            .iter()
            .filter(other)
            .find(|r| Some(&r.id) == hard)
            .or_else(|| settings.routes.iter().find(other));
        if found.is_some() {
            return found;
        }
    }
    settings
        .routes
        .iter()
        .find(|r| Some(&r.id) == hard && r.id != implementer.id)
        .or_else(|| {
            settings
                .routes
                .iter()
                .find(|r| r.harness != implementer.harness)
        })
}

const MAX_REVIEW_SCREENSHOTS: usize = 8;

/// Images under the worktree's `artifacts/` (gitignored, so never in the
/// diff) written since `since_ms`, newest first.
pub(super) fn attempt_screenshots(worktree: &Path, since_ms: i64) -> Vec<PathBuf> {
    let mut found: Vec<(i64, PathBuf)> = Vec::new();
    let mut dirs = vec![worktree.join("artifacts")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(path);
                continue;
            }
            let is_image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "webp"
                )
            });
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            if is_image && modified >= since_ms {
                found.push((modified, path));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
        .into_iter()
        .take(MAX_REVIEW_SCREENSHOTS)
        .map(|(_, p)| p)
        .collect()
}

/// The run request of a read-only session: review's, and anything that must
/// be held to the same restrictions (no edits, no network, never resumed,
/// no MCP servers but the empty config it is handed).
pub(super) fn review_request<'a>(
    route: &'a Route,
    worktree: &'a Path,
    mcp_config: &'a Path,
    settings_path: &'a Path,
    images: &'a [PathBuf],
) -> harness::RunRequest<'a> {
    harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        max_budget_usd: None,
        resume: None,
        review: true,
        mcp_config: Some(mcp_config),
        settings_path: Some(settings_path),
        network_allowed: false,
        codex_mcp: None,
        images,
        repo_settings: true,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_review(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    task: &Task,
    worktree: &Path,
    base_sha: &str,
    verify_results: &[VerifyOutcome],
    implementer_note: &str,
    review_route: &Route,
    deny_read: &[String],
    cancel: &CancelToken,
    cost_usd: &mut f64,
    fingerprint: &mut Option<Fingerprint>,
) -> Result<ReviewResult, RunError> {
    let wt = worktree.to_path_buf();
    let base = base_sha.to_string();
    let diff =
        tokio::task::spawn_blocking(move || git::diff_full(&wt, &base, 60_000).unwrap_or_default())
            .await
            .unwrap_or_default();

    let mut brief_text = String::new();
    brief_text.push_str("## Review\n\n");
    brief_text.push_str(&task.goal);
    brief_text.push_str("\n\n## Acceptance criteria\n\n");
    for c in &task.criteria {
        brief_text.push_str("- ");
        brief_text.push_str(c);
        brief_text.push('\n');
    }
    // Blind review: the implementer's account is left out entirely.
    if !task.variant().review_blind {
        brief_text.push_str("\n## Implementer\n\n");
        brief_text.push_str(implementer_note);
    }
    brief_text.push_str("\n\nCriteria marked \"Checked by review\" have no command behind them: check them from the diff and the repository yourself.\n\nThe repository's process rules about commits, pull requests, release notes and lesson or changelog files belong to the orchestrator, not this task: judge the change against the task and its criteria, and do not fail it for those.\n");
    let evidence = task.variant().review_evidence;
    brief_text.push_str("\n## Verify results\n\n");
    for v in verify_results {
        brief_text.push_str(&format!(
            "- `{}` -> exit {:?}\n```\n{}\n```\n",
            v.command,
            v.code,
            tail_chars(v.tail.trim(), if evidence { 3000 } else { 800 })
        ));
    }
    let images = if evidence {
        let since = task
            .attempts
            .iter()
            .find(|a| a.n == attempt_n && a.stage == Stage::Implement)
            .map_or(0, |a| a.started_at);
        attempt_screenshots(worktree, since)
    } else {
        Vec::new()
    };
    if !images.is_empty() {
        brief_text.push_str("\n## Screenshots\n\nSaved by this attempt. Open each one and check it against the criteria it is meant to prove; a screenshot that does not show what a criterion claims is a finding.\n\n");
        for image in &images {
            let shown = image.strip_prefix(worktree).unwrap_or(image);
            brief_text.push_str(&format!("- `{}`\n", shown.display()));
        }
    }
    brief_text.push_str("\n## Diff\n\n```diff\n");
    brief_text.push_str(&diff);
    brief_text.push_str("\n```\n\n## Report format\n\nReply with:\n\n```sushi-review\n{\"verdict\":\"PASS|FAIL\",\"findings\":[]}\n```\n");
    if task.variant().contract {
        brief_text.push('\n');
        brief_text.push_str(brief::REVIEW_CONTRACT);
        brief_text.push('\n');
    }

    // Its own subdirectory: a review's events/brief/settings must never
    // land in the implement attempt's `runs/<n>/` files.
    let run_dir = app.store.run_dir(task_id, attempt_n).join("review");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let settings_snapshot = app.settings.read().unwrap().clone();
    let settings_path = run_dir.join("settings.json");
    if matches!(review_route.harness, Harness::Claude) {
        // No token is ever registered for a review run (it's read-only and
        // never gated), so it must not get a Stop hook either -- installing
        // one would just be a guaranteed fail-open round trip.
        let claude_settings = harness::build_claude_settings(
            None,
            settings_snapshot.sandbox,
            &settings_snapshot.allowed_domains,
            deny_read,
            None,
        );
        let _ = store::write_json_atomic(&settings_path, &claude_settings);
    }

    let req = review_request(review_route, worktree, &mcp_path, &settings_path, &images);
    match run_harness(
        app,
        task_id,
        attempt_n,
        false,
        worktree,
        &req,
        &brief_text,
        &events_path,
        cancel,
        None,
        None,
    )
    .await
    {
        Ok(outcome) => {
            *cost_usd += outcome.cost_usd.unwrap_or(0.0);
            *fingerprint = outcome.fingerprint.clone();
            let text = outcome.final_text.unwrap_or_default();
            if let Some(result) = brief::parse_review(&text) {
                return Ok(result);
            }
            Err(RunError::Io(match outcome.error {
                Some(error) => format!("review did not run ({error})"),
                None => format!(
                    "review reply had no sushi-review verdict: {}",
                    tail_chars(text.trim(), 600)
                ),
            }))
        }
        Err(RunError::Cancelled) => {
            // Stopped mid-review: what it streamed so far is still spent.
            *fingerprint = read_cancelled_fingerprint(&events_path);
            *cost_usd += replay_run_cost(&events_path, &settings_snapshot.prices)
                .and_then(|o| o.cost_usd)
                .unwrap_or(0.0);
            Err(RunError::Cancelled)
        }
        Err(e) => Err(e),
    }
}

/// Writes `settings.json` for a read-only Claude session (no Stop hook --
/// used by the plan stage and orchestrator triage, never the implement
/// path, which additionally wires one up) that still carries its route's
/// profile (env/API key) the same way an implement attempt would. A no-op
/// for a Codex route, which has no such settings file.
pub(super) fn write_readonly_claude_settings_with_profile(
    route: &Route,
    app: &App,
    key_path: &Path,
    settings: &Settings,
    deny_read: &[String],
    settings_path: &Path,
) {
    let profile = route
        .profile_id
        .as_ref()
        .and_then(|pid| app.secrets.read().unwrap().profiles.get(pid).cloned());
    let mut profile_obj = serde_json::Map::new();
    if let Some(p) = &profile {
        if !p.env.is_empty() {
            profile_obj.insert(
                "env".to_string(),
                serde_json::to_value(&p.env).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(key) = &p.key {
            if store::write_secret_file(key_path, key).is_ok() {
                profile_obj.insert(
                    "apiKeyHelper".to_string(),
                    serde_json::Value::String(format!(
                        "cat {}",
                        harness::shell_quote(&key_path.to_string_lossy())
                    )),
                );
            }
        }
    }
    let profile_value = if profile_obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(profile_obj))
    };
    let claude_settings = harness::build_claude_settings(
        profile_value.as_ref(),
        settings.sandbox,
        &settings.allowed_domains,
        deny_read,
        None,
    );
    let _ = store::write_json_atomic(settings_path, &claude_settings);
}
