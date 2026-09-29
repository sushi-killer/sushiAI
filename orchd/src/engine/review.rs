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
/// diff) written since `since_ms`, newest first, at most
/// `MAX_REVIEW_SCREENSHOTS`.
pub(super) fn attempt_screenshots(worktree: &Path, since_ms: i64) -> Vec<PathBuf> {
    let mut found = attempt_images(worktree, since_ms);
    found.truncate(MAX_REVIEW_SCREENSHOTS);
    found
}

/// Copies every image the attempt wrote under `artifacts/` into `evidence_dir`
/// (flattened names), so it survives the worktree. Returns the copies.
pub(super) fn save_evidence(worktree: &Path, since_ms: i64, evidence_dir: &Path) -> Vec<String> {
    let images = attempt_images(worktree, since_ms);
    if images.is_empty() || std::fs::create_dir_all(evidence_dir).is_err() {
        return Vec::new();
    }
    let root = worktree.join("artifacts");
    let mut saved = Vec::new();
    for image in images {
        let rel = image.strip_prefix(&root).unwrap_or(&image);
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("_");
        let target = evidence_dir.join(name);
        if std::fs::copy(&image, &target).is_ok() {
            saved.push(target.display().to_string());
        }
    }
    saved.sort();
    saved
}

fn attempt_images(worktree: &Path, since_ms: i64) -> Vec<PathBuf> {
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
            let is_image = crate::model::image_mime(&path).is_some();
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
    found.into_iter().map(|(_, p)| p).collect()
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
    agent_decisions: &[String],
    review_route: &Route,
    deny_read: &[String],
    cancel: &CancelToken,
    cost_usd: &mut f64,
    fingerprint: &mut Option<Fingerprint>,
) -> Result<(ReviewResult, bool), RunError> {
    let wt = worktree.to_path_buf();
    let base = base_sha.to_string();
    let diff =
        tokio::task::spawn_blocking(move || git::diff_full(&wt, &base, 60_000).unwrap_or_default())
            .await
            .unwrap_or_default();

    let evidence = task.variant().review_evidence;
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
    // The saved copies outlive the worktree, so they are what the reviewer
    // is pointed at; the worktree paths only when nothing was copied.
    let saved = task
        .attempts
        .iter()
        .find(|a| a.n == attempt_n && a.stage == Stage::Implement)
        .map(|a| a.evidence.clone())
        .unwrap_or_default();
    let shown: Vec<String> = if saved.is_empty() {
        images
            .iter()
            .map(|image| {
                image
                    .strip_prefix(worktree)
                    .unwrap_or(image)
                    .display()
                    .to_string()
            })
            .collect()
    } else {
        saved
    };
    let brief_text = brief::build_review_brief(
        task,
        implementer_note,
        agent_decisions,
        verify_results,
        &shown,
        &diff,
    );

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
        CostTag::task("review", &review_route.id),
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
            if let Some(result) = brief::parse_review_with_rule(&text) {
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
