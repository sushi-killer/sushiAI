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

/// The weakest strength a reviewer or judge may have: never below the
/// implementer's, and at least 2 for mechanical/standard work, 3 for hard.
pub(super) fn review_floor(implementer: &Route, tier: Tier) -> u32 {
    let tier_floor = if tier == Tier::Hard { 3 } else { 2 };
    route_strength(implementer).max(tier_floor)
}

/// The cheapest route at `floor` or above, skipping `exclude`; with none, the
/// strongest one (cheapest first) when `fallback` is set. Unpriced routes
/// sort after priced ones. Ties go to a route on a harness other than
/// `other_than`'s, then to `own_id`, then settings order. The bool is true
/// when the route reaches the floor.
pub(super) fn cheapest_route_at<'a>(
    settings: &'a Settings,
    floor: u32,
    exclude: Option<&str>,
    other_than: &Route,
    own_id: Option<&str>,
    fallback: bool,
) -> Option<(&'a Route, bool)> {
    let pool: Vec<(usize, &Route)> = settings
        .routes
        .iter()
        .enumerate()
        .filter(|(_, r)| Some(r.id.as_str()) != exclude)
        .collect();
    let key = |(i, r): &(usize, &Route)| {
        let cost = route_cost(&settings.prices, r);
        (
            cost.is_none(),
            cost.unwrap_or(0.0),
            r.harness == other_than.harness,
            Some(r.id.as_str()) != own_id,
            *i,
        )
    };
    let cmp = |a: &(usize, &Route), b: &(usize, &Route)| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0)
            .then(ka.1.total_cmp(&kb.1))
            .then(ka.2.cmp(&kb.2))
            .then(ka.3.cmp(&kb.3))
            .then(ka.4.cmp(&kb.4))
    };
    if let Some(found) = pool
        .iter()
        .filter(|(_, r)| route_strength(r) >= floor)
        .min_by(|a, b| cmp(a, b))
    {
        return Some((found.1, true));
    }
    if !fallback {
        return None;
    }
    let top = pool.iter().map(|(_, r)| route_strength(r)).max()?;
    pool.iter()
        .filter(|(_, r)| route_strength(r) == top)
        .min_by(|a, b| cmp(a, b))
        .map(|(_, r)| (*r, false))
}

/// The review route and a one-line reason. An explicit `review` id is used
/// as is. `"auto"` picks the cheapest route no weaker than
/// `review_floor` (ties: another harness, then the implementer's own route),
/// else the strongest one. `""` -> no review (handled by the caller).
pub fn select_review_route<'a>(
    settings: &'a Settings,
    implementer: &Route,
    tier: Tier,
) -> Option<(&'a Route, String)> {
    if settings.review != "auto" {
        let route = settings.routes.iter().find(|r| r.id == settings.review)?;
        return Some((route, "explicit setting".to_string()));
    }
    let floor = review_floor(implementer, tier);
    let (route, reached) = cheapest_route_at(
        settings,
        floor,
        None,
        implementer,
        Some(&implementer.id),
        true,
    )?;
    let reason = if reached {
        format!(
            "{} tier, strength {}",
            tier_name(tier),
            route_strength(route)
        )
    } else {
        format!("no route at strength {floor}; strongest available")
    };
    Some((route, reason))
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Mechanical => "mechanical",
        Tier::Standard => "standard",
        Tier::Hard => "hard",
    }
}

const MAX_REVIEW_SCREENSHOTS: usize = 8;

/// Which images an attempt's evidence may hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceScope {
    /// Every image the attempt wrote: some visual criterion names no path.
    All,
    /// The task has no visual criterion: no image is evidence.
    Nothing,
    /// Only images matching these worktree-relative globs (`artifacts/<file>`,
    /// or `artifacts/<dir>/*` for a directory a check names).
    Named(Vec<String>),
}

/// The scope a task's visual criteria set.
pub fn evidence_scope(task: &Task) -> EvidenceScope {
    let visual = task.visual_criteria_texts();
    if visual.is_empty() {
        return EvidenceScope::Nothing;
    }
    let mut named = Vec::new();
    for criterion in visual {
        let found = named_artifact_paths(criterion);
        if found.is_empty() {
            return EvidenceScope::All;
        }
        for entry in found {
            if !named.contains(&entry) {
                named.push(entry);
            }
        }
    }
    EvidenceScope::Named(named)
}

/// The `artifacts/...` paths a criterion names: an image file as is, anything
/// else as a directory (`artifacts/<dir>/*`). A bare `artifacts/` names none.
fn named_artifact_paths(criterion: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in criterion.split_whitespace() {
        let token = token.trim_matches(|c: char| {
            !c.is_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-' | '*' | '?')
        });
        let token = token.trim_start_matches("./");
        let Some(rest) = token.strip_prefix("artifacts/") else {
            continue;
        };
        let rest = rest.trim_end_matches('.');
        if rest.is_empty() || rest.trim_matches('/').is_empty() {
            continue;
        }
        let entry = if crate::model::image_mime(Path::new(rest)).is_some() {
            token.trim_end_matches('.').to_string()
        } else {
            format!("artifacts/{}/*", rest.trim_end_matches('/'))
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// The name `save_evidence` gives a copy of an `artifacts/<rel>` entry.
fn flat_name(entry: &str) -> String {
    let rel = entry.strip_prefix("artifacts/").unwrap_or(entry);
    rel.split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

impl EvidenceScope {
    /// Whether the image at `rel` (relative to `artifacts/`) is evidence.
    pub fn allows(&self, rel: &str) -> bool {
        match self {
            EvidenceScope::All => true,
            EvidenceScope::Nothing => false,
            EvidenceScope::Named(entries) => {
                let full = format!("artifacts/{rel}");
                entries.iter().any(|e| glob_match(e, &full))
            }
        }
    }

    /// Whether a saved copy (by its flattened file name) is evidence.
    pub fn allows_saved(&self, saved: &str) -> bool {
        match self {
            EvidenceScope::All => true,
            EvidenceScope::Nothing => false,
            EvidenceScope::Named(entries) => {
                let name = Path::new(saved)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                entries.iter().any(|e| glob_match(&flat_name(e), &name))
            }
        }
    }

    /// The named entries no saved copy in `evidence` matches.
    pub fn missing(&self, evidence: &[String]) -> Vec<String> {
        let EvidenceScope::Named(entries) = self else {
            return Vec::new();
        };
        let names: Vec<String> = evidence
            .iter()
            .filter_map(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        entries
            .iter()
            .filter(|e| !names.iter().any(|n| glob_match(&flat_name(e), n)))
            .map(|e| {
                e.strip_suffix('*')
                    .filter(|d| d.ends_with('/'))
                    .unwrap_or(e)
                    .to_string()
            })
            .collect()
    }
}

/// Images under the worktree's `artifacts/` (gitignored, so never in the
/// diff) written since `since_ms` and inside `scope`, newest first, at most
/// `MAX_REVIEW_SCREENSHOTS`.
pub(super) fn attempt_screenshots(
    worktree: &Path,
    since_ms: i64,
    scope: &EvidenceScope,
) -> Vec<PathBuf> {
    let mut found = attempt_images(worktree, since_ms, scope);
    found.truncate(MAX_REVIEW_SCREENSHOTS);
    found
}

/// True for a path that is a test file: any directory component named `test`,
/// `tests`, `__tests__`, `spec`, `specs` or `e2e`, or a file name with
/// `.test.`, `.spec.` or `_test.` in it, or starting with `test_`.
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let Some((name, dirs)) = parts.split_last() else {
        return false;
    };
    let is_dir_name =
        |p: &str| matches!(p, "test" | "tests" | "__tests__" | "spec" | "specs" | "e2e");
    dirs.iter().any(|d| is_dir_name(d))
        || is_dir_name(name)
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
}

/// Copies the images the attempt wrote under `artifacts/` that `scope` allows
/// into `evidence_dir` (flattened names), so they survive the worktree.
/// Returns the copies.
pub(super) fn save_evidence(
    worktree: &Path,
    since_ms: i64,
    evidence_dir: &Path,
    scope: &EvidenceScope,
) -> Vec<String> {
    let images = attempt_images(worktree, since_ms, scope);
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

fn attempt_images(worktree: &Path, since_ms: i64, scope: &EvidenceScope) -> Vec<PathBuf> {
    let root = worktree.join("artifacts");
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
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let is_image = crate::model::image_mime(&path).is_some() && scope.allows(&rel);
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
    found.sort_by_key(|item| std::cmp::Reverse(item.0));
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
        review: true,
        mcp_config: Some(mcp_config),
        settings_path: Some(settings_path),
        network_allowed: false,
        codex_mcp: &[],
        images,
        repo_settings: true,
    }
}

/// Why a review run produced no verdict.
pub(super) enum ReviewFailure {
    Cancelled,
    /// The harness did not run or errored.
    Harness(String),
    /// The run finished, but its reply had no parseable verdict.
    NoVerdict(String),
}

/// The start of a reply, at most `max` characters, with an ellipsis when cut.
pub(super) fn head_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

/// Review run `round` of an attempt (1 = the first). A re-run after a reply
/// without a verdict (`after_no_verdict`) carries a note, and every run after
/// the first writes to its own `review-<round>` subdirectory so the earlier
/// reply stays on disk.
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
    round: u32,
    after_no_verdict: bool,
    dropped: &[(String, String)],
) -> Result<(ReviewResult, bool), ReviewFailure> {
    let wt = worktree.to_path_buf();
    let base = base_sha.to_string();
    let diff =
        tokio::task::spawn_blocking(move || git::diff_full(&wt, &base, 60_000).unwrap_or_default())
            .await
            .unwrap_or_default();

    let evidence = task.variant().review_evidence;
    let this_attempt = task
        .attempts
        .iter()
        .find(|a| a.n == attempt_n && a.stage == Stage::Implement);
    let evidence_from = this_attempt.and_then(|a| a.evidence_from);
    let images = if !evidence {
        Vec::new()
    } else if evidence_from.is_some() {
        // Reused evidence: the saved copies, not what the worktree holds.
        this_attempt
            .map(|a| a.evidence.iter().map(PathBuf::from).collect())
            .unwrap_or_default()
    } else {
        attempt_screenshots(
            worktree,
            this_attempt.map_or(0, |a| a.started_at),
            &evidence_scope(task),
        )
    };
    // The saved copies outlive the worktree, so reviewers can open them.
    let saved = this_attempt.map(|a| a.evidence.clone()).unwrap_or_default();
    let source_attempt = evidence_from.unwrap_or(attempt_n);
    let shown: Vec<(String, Option<u32>)> = if saved.is_empty() {
        images
            .iter()
            .map(|image| {
                let rel = image.strip_prefix(worktree).unwrap_or(image);
                (rel.display().to_string(), Some(attempt_n))
            })
            .collect()
    } else {
        saved
            .into_iter()
            .map(|s| (s, Some(source_attempt)))
            .collect()
    };
    let mut brief_text = brief::build_review_brief(
        task,
        implementer_note,
        agent_decisions,
        verify_results,
        &shown,
        evidence_from,
        &diff,
    );
    brief_text.push_str(&brief::dropped_findings_block(dropped));
    if after_no_verdict {
        brief_text.push('\n');
        brief_text.push_str(&brief::review_retry_note());
    }

    // Its own subdirectory: a review's events/brief/settings must never
    // land in the implement attempt's `runs/<n>/` files.
    let run_dir = app.store.run_dir(task_id, attempt_n).join(if round <= 1 {
        "review".to_string()
    } else {
        format!("review-{round}")
    });
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
            Err(match outcome.error {
                Some(error) => ReviewFailure::Harness(format!("review did not run ({error})")),
                None => ReviewFailure::NoVerdict(format!(
                    "review reply had no sushi-review verdict: {}",
                    head_chars(text.trim(), 300)
                )),
            })
        }
        Err(RunError::Cancelled) => {
            // Stopped mid-review: what it streamed so far is still spent.
            *fingerprint = read_cancelled_fingerprint(&events_path);
            *cost_usd += replay_run_cost(&events_path, &settings_snapshot.prices)
                .and_then(|o| o.cost_usd)
                .unwrap_or(0.0);
            Err(ReviewFailure::Cancelled)
        }
        Err(RunError::Io(why) | RunError::NotFound(why)) => Err(ReviewFailure::Harness(why)),
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
    if let Some(account) = route
        .account_id
        .as_ref()
        .and_then(|id| app.secrets.read().unwrap().accounts.get(id).cloned())
    {
        if account.kind == "apiKey" {
            if store::write_secret_file(key_path, &account.value).is_ok() {
                profile_obj.insert(
                    "apiKeyHelper".into(),
                    serde_json::Value::String(format!(
                        "cat {}",
                        harness::shell_quote(&key_path.to_string_lossy())
                    )),
                );
            }
        } else if account.kind == "subscription" {
            let _ =
                store::write_secret_file(&key_path.with_file_name("oauth-token"), &account.value);
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

#[cfg(test)]
mod tests {
    use super::head_chars;

    #[test]
    fn a_long_reply_is_quoted_from_its_start() {
        let reply = format!("START{}END", "x".repeat(600));
        let quote = head_chars(&reply, 300);
        assert!(quote.starts_with("STARTxxx"));
        assert!(!quote.contains("END"));
        assert!(quote.ends_with('\u{2026}'));
        assert_eq!(quote.chars().count(), 301);
        assert_eq!(head_chars("short", 300), "short");
    }
}

#[cfg(test)]
mod is_test_path_tests {
    use super::is_test_path;

    #[test]
    fn test_directories_and_file_names_count() {
        for path in [
            "test/a.rs",
            "src/tests/a.rs",
            "web/__tests__/a.ts",
            "spec/a.rb",
            "src/specs/a.rb",
            "e2e/login.ts",
            "src/panel.test.ts",
            "src/panel.spec.ts",
            "src/panel_test.go",
            "tools/test_panel.py",
        ] {
            assert!(is_test_path(path), "{path}");
        }
    }

    #[test]
    fn near_misses_do_not_count() {
        for path in [
            "src/contest.rs",
            "testing/x.rs",
            "src/latest.rs",
            "src/attest_all.rs",
            "src/testimony.rs",
            "specification/a.md",
            "",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }
}
