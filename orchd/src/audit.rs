//! `repo.audit`: one read-only harness run in a repository's own checkout
//! that grades it against the fixed agent-readiness rubric in
//! `brief::AUDIT_RUBRIC` and hands back a ```sushi-audit report. The
//! judgement is the model's; this module only starts the run, under the
//! same restrictions as a review, and stores what came back. Nothing here
//! knows anything about the repository being audited.

use super::*;

fn audit_id_param(app: &App, params: &serde_json::Value) -> Result<String, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id is required")?;
    uuid::Uuid::parse_str(id).map_err(|_| "invalid audit id".to_string())?;
    let audits_root = app.store.data_dir().join("audits");
    if !app.store.audit_dir(id).starts_with(&audits_root) {
        return Err("invalid audit id".to_string());
    }
    Ok(id.to_string())
}

/// An audit as the protocol shows it: the record, with its report (or
/// `null` when there is none) alongside.
fn audit_json(app: &App, audit: &Audit) -> serde_json::Value {
    let mut v = serde_json::to_value(audit).unwrap_or_else(|_| json!({}));
    let report = app.store.load_audit_report(&audit.id).ok().flatten();
    v["report"] = serde_json::to_value(report).unwrap_or(serde_json::Value::Null);
    v
}

pub(super) fn broadcast(app: &App, audit: &Audit) {
    let _ = app.events_tx.send(Event::Audit {
        audit: Box::new(audit_json(app, audit)),
    });
}

/// `repo.audit {repo, route?}`: records the audit, starts its run in the
/// background and returns the record (with its `id`) right away. `route`
/// defaults to the planner's route; an unknown one is refused rather than
/// swapped for another.
/// The repo param resolved to its git top level: an audit reads a
/// repository, never an arbitrary directory such as `/` or `$HOME`, and one
/// repository is listed under one path however it was spelled.
async fn audited_repo(params: &serde_json::Value) -> Result<String, String> {
    let repo = chat::repo_param(params)?;
    tokio::task::spawn_blocking(move || git::repo_toplevel(Path::new(&repo)))
        .await
        .map_err(|e| e.to_string())?
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|_| "repo is not a git repository".to_string())
}

pub async fn handle_start(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = audited_repo(&params).await?;
    let settings = app.settings.read().unwrap().clone();
    let route_id = params
        .get("route")
        .and_then(|v| v.as_str())
        .filter(|r| !r.is_empty())
        .unwrap_or(&settings.planner)
        .to_string();
    let route = settings
        .routes
        .iter()
        .find(|r| r.id == route_id)
        .cloned()
        .ok_or_else(|| format!("no route {route_id:?} is configured"))?;

    let audit = Audit {
        id: uuid::Uuid::new_v4().to_string(),
        repo,
        route_id: route.id.clone(),
        harness: route.harness,
        model: route.model.clone().unwrap_or_default(),
        status: AuditStatus::Running,
        started_at: now_ms(),
        ended_at: None,
        error: None,
        usage: None,
        cost_usd: 0.0,
        fingerprint: None,
    };
    let dir = app.store.audit_dir(&audit.id);
    let brief_text = brief::build_audit_brief();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("brief.md"), &brief_text).map_err(|e| e.to_string())?;
    app.store.save_audit(&audit).map_err(|e| e.to_string())?;

    let cancel = CancelToken::new();
    app.audits
        .lock()
        .unwrap()
        .insert(audit.id.clone(), cancel.clone());
    broadcast(app, &audit);
    let result = audit_json(app, &audit);
    let app = app.arc();
    tokio::spawn(async move {
        run(&app, audit, route, brief_text, cancel).await;
    });
    Ok(result)
}

pub async fn handle_get(app: &App, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let id = audit_id_param(app, &params)?;
    let audit = app
        .store
        .load_audit(&id)
        .map_err(|e| e.to_string())?
        .ok_or("no such audit")?;
    Ok(audit_json(app, &audit))
}

/// Every audit of `repo`, newest first, each with its report.
pub async fn handle_list(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = audited_repo(&params).await?;
    let audits = app.store.list_audits(&repo).map_err(|e| e.to_string())?;
    Ok(serde_json::Value::Array(
        audits.iter().map(|a| audit_json(app, a)).collect(),
    ))
}

/// A review run's read-only request, minus the audited checkout's own
/// Claude settings: orchd does not own that checkout, and its settings'
/// hooks are shell commands that would run outside the sandbox, in it.
pub(super) fn audit_request<'a>(
    route: &'a Route,
    repo: &'a Path,
    mcp: &'a Path,
    settings_path: &'a Path,
) -> harness::RunRequest<'a> {
    harness::RunRequest {
        repo_settings: false,
        ..review_request(route, repo, mcp, settings_path, &[])
    }
}

/// Writes the run's empty MCP config and, for Claude, the same read-only
/// `settings.json` a review gets (plus the route's profile, as the planner
/// route may carry one). Returns the MCP config, settings and key paths.
pub(super) fn prepare_run(
    app: &App,
    route: &Route,
    dir: &Path,
    settings: &Settings,
) -> (PathBuf, PathBuf, PathBuf) {
    let mcp_path = dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = dir.join("settings.json");
    let key_path = dir.join("key");
    if matches!(route.harness, Harness::Claude) {
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        write_readonly_claude_settings_with_profile(
            route,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    (mcp_path, settings_path, key_path)
}

async fn run(
    app: &Arc<App>,
    mut audit: Audit,
    route: Route,
    brief_text: String,
    cancel: CancelToken,
) {
    let dir = app.store.audit_dir(&audit.id);
    // A parallel slot, like every other harness run.
    let permit = tokio::select! {
        _ = cancel.cancelled() => None,
        p = app.slots.clone().acquire_owned() => p.ok(),
    };
    let result = match permit {
        Some(_permit) => {
            let settings = app.settings.read().unwrap().clone();
            let (mcp_path, settings_path, key_path) = prepare_run(app, &route, &dir, &settings);
            let repo = PathBuf::from(&audit.repo);
            let req = audit_request(&route, &repo, &mcp_path, &settings_path);
            let events_path = dir.join("events.jsonl");
            let result = run_harness(
                app,
                &audit.id,
                0,
                false,
                &repo,
                &req,
                CostTag::repo("audit", &route.id, &audit.repo),
                &brief_text,
                &events_path,
                &cancel,
                None,
                None,
            )
            .await;
            let _ = std::fs::remove_file(&key_path);
            if matches!(result, Err(RunError::Cancelled)) {
                audit.fingerprint = read_cancelled_fingerprint(&events_path);
            }
            result
        }
        None => Err(RunError::Cancelled),
    };
    finish(app, &mut audit, result);
    app.audits.lock().unwrap().remove(&audit.id);
    broadcast(app, &audit);
}

/// Records how the run ended. A report is stored only when the reply held
/// a parsable one; otherwise the audit fails with the reason and no report.
fn finish(app: &App, audit: &mut Audit, result: Result<harness::RunOutcome, RunError>) {
    audit.ended_at = Some(now_ms());
    match result {
        Ok(outcome) => {
            audit.cost_usd = outcome.cost_usd.unwrap_or(0.0);
            audit.fingerprint = outcome.fingerprint.clone();
            audit.usage = Some(Usage {
                input: outcome.usage_input,
                output: outcome.usage_output,
                cached: outcome.usage_cached,
            });
            let text = outcome.final_text.unwrap_or_default();
            match brief::parse_audit(&text) {
                Ok(report) => match app.store.save_audit_report(&audit.id, &report) {
                    Ok(()) => audit.status = AuditStatus::Done,
                    Err(e) => {
                        audit.status = AuditStatus::Failed;
                        audit.error = Some(format!("could not store the report: {e}"));
                    }
                },
                Err(parse_error) => {
                    audit.status = AuditStatus::Failed;
                    audit.error = Some(match (outcome.error, parse_error) {
                        (Some(error), _) => format!("audit did not run ({error})"),
                        (None, brief::AuditParseError::NoBlock) => format!(
                            "audit reply had no sushi-audit report: {}",
                            tail_chars(text.trim(), 600)
                        ),
                        (None, brief::AuditParseError::Invalid(reason)) => {
                            format!("audit reply's sushi-audit report is invalid: {reason}")
                        }
                    });
                }
            }
        }
        Err(RunError::Cancelled) => {
            audit.status = AuditStatus::Stopped;
            audit.error = Some("Stopped.".to_string());
        }
        Err(RunError::Io(error)) => {
            audit.status = AuditStatus::Failed;
            audit.error = Some(error);
        }
    }
    let _ = app.store.save_audit(audit);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(harness: Harness) -> Route {
        Route {
            id: "r".into(),
            label: "R".into(),
            harness,
            model: Some("m".into()),
            effort: Some("high".into()),
            profile_id: None,
            strength: None,
        }
    }

    fn test_app() -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            dir.path().join("data"),
            dir.path().join("orchd.sock"),
            "orchd".to_string(),
        )
        .unwrap();
        (app, dir)
    }

    #[test]
    fn the_audit_run_has_a_review_run_s_read_only_argv_without_repo_settings() {
        let (app, dir) = test_app();
        let settings = Settings::default();
        let repo = dir.path().join("repo");
        for harness in [Harness::Claude, Harness::Codex] {
            let route = route(harness);
            let run_dir = dir.path().join(format!("{harness:?}"));
            std::fs::create_dir_all(&run_dir).unwrap();
            let (mcp, settings_path, _) = prepare_run(&app, &route, &run_dir, &settings);
            let argv = harness::build_argv(&audit_request(&route, &repo, &mcp, &settings_path));
            // What run_review hands the harness, for the same route.
            let review = harness::build_argv(&harness::RunRequest {
                harness,
                worktree: &repo,
                model: Some("m"),
                effort: Some("high"),
                max_budget_usd: None,
                review: true,
                mcp_config: Some(&mcp),
                settings_path: Some(&settings_path),
                network_allowed: false,
                codex_mcp: None,
                images: &[],
                repo_settings: false,
            });
            assert_eq!(argv, review, "{harness:?}");
            assert!(!argv.iter().any(|a| a == "acceptEdits"
                || a == "workspace-write"
                || a.contains("network_access")
                || a == "--resume"));
            match harness {
                Harness::Claude => {
                    // Never the audited repo's own settings: their hooks run
                    // outside the sandbox.
                    assert!(argv
                        .windows(2)
                        .any(|w| w[0] == "--setting-sources" && w[1].is_empty()));
                    assert!(argv
                        .windows(2)
                        .any(|w| w[0] == "--tools" && w[1] == "Read,Grep,Glob"));
                    assert!(argv
                        .windows(2)
                        .any(|w| w[0] == "--permission-mode" && w[1] == "plan"));
                }
                Harness::Codex => assert!(argv
                    .windows(2)
                    .any(|w| w[0] == "--sandbox" && w[1] == "read-only")),
            }
            let mcp_text = std::fs::read_to_string(&mcp).unwrap();
            assert_eq!(mcp_text, r#"{"mcpServers":{}}"#);
        }
    }

    #[test]
    fn the_audit_run_gets_a_review_run_s_claude_settings() {
        let (app, dir) = test_app();
        let settings = Settings::default();
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let (_, settings_path, _) = prepare_run(&app, &route(Harness::Claude), &run_dir, &settings);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        // What run_review writes: no profile, the daemon's data dir denied,
        // no Stop hook.
        let review = harness::build_claude_settings(
            None,
            settings.sandbox,
            &settings.allowed_domains,
            &[app.data_dir.to_string_lossy().to_string()],
            None,
        );
        assert_eq!(written, review);
        assert!(written.get("hooks").is_none());

        // A Codex route has no settings file at all, same as a review.
        let codex_dir = dir.path().join("codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let (_, codex_settings, _) =
            prepare_run(&app, &route(Harness::Codex), &codex_dir, &settings);
        assert!(!codex_settings.exists());
    }

    #[tokio::test]
    async fn repo_audit_refuses_a_bad_repo_or_route_and_get_a_bad_id() {
        let (app, dir) = test_app();
        let err = app
            .dispatch("repo.audit", json!({"repo": "relative/path"}))
            .await
            .unwrap_err();
        assert!(err.contains("absolute"), "{err}");
        let plain = dir.path().to_string_lossy().to_string();
        let err = app
            .dispatch("repo.audit", json!({"repo": plain}))
            .await
            .unwrap_err();
        assert_eq!(err, "repo is not a git repository");
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo_dir)
            .status()
            .unwrap();
        let repo = repo_dir.to_string_lossy().to_string();
        let err = app
            .dispatch("repo.audit", json!({"repo": repo, "route": "gone"}))
            .await
            .unwrap_err();
        assert!(err.contains("gone"), "{err}");
        assert_eq!(
            app.dispatch("repo.audit.list", json!({"repo": repo}))
                .await
                .unwrap(),
            json!([])
        );
        let err = app
            .dispatch("repo.audit.get", json!({"id": "../tasks"}))
            .await
            .unwrap_err();
        assert_eq!(err, "invalid audit id");
    }

    #[test]
    fn a_reply_without_a_report_fails_the_audit_and_stores_none() {
        let (app, _dir) = test_app();
        let mut audit = Audit {
            id: uuid::Uuid::new_v4().to_string(),
            repo: "/repo".into(),
            route_id: "r".into(),
            harness: Harness::Claude,
            model: "m".into(),
            status: AuditStatus::Running,
            started_at: 1,
            ended_at: None,
            error: None,
            usage: None,
            cost_usd: 0.0,
            fingerprint: None,
        };
        let outcome = harness::RunOutcome {
            final_text: Some("Everything looks great.".into()),
            cost_usd: Some(0.25),
            ..Default::default()
        };
        finish(&app, &mut audit, Ok(outcome));
        assert_eq!(audit.status, AuditStatus::Failed);
        assert!(audit.error.as_deref().unwrap().contains("no sushi-audit"));
        assert_eq!(audit.cost_usd, 0.25);
        assert!(app.store.load_audit_report(&audit.id).unwrap().is_none());
        let stored = app.store.load_audit(&audit.id).unwrap().unwrap();
        assert_eq!(stored.status, AuditStatus::Failed);
    }

    #[test]
    fn an_incomplete_report_fails_the_audit_with_the_reason_and_stores_none() {
        let (app, _dir) = test_app();
        let mut audit = Audit {
            id: uuid::Uuid::new_v4().to_string(),
            repo: "/repo".into(),
            route_id: "r".into(),
            harness: Harness::Claude,
            model: "m".into(),
            status: AuditStatus::Running,
            started_at: 1,
            ended_at: None,
            error: None,
            usage: None,
            cost_usd: 0.0,
            fingerprint: None,
        };
        let reply = "```sushi-audit\n{\"summary\":\"ok\",\"items\":[{\"area\":\"x\",\"grade\":\"good\",\"effort\":\"small\"}],\"topFixes\":[]}\n```";
        let outcome = harness::RunOutcome {
            final_text: Some(reply.into()),
            ..Default::default()
        };
        finish(&app, &mut audit, Ok(outcome));
        assert_eq!(audit.status, AuditStatus::Failed);
        let error = audit.error.as_deref().unwrap();
        assert!(error.contains("report is invalid"), "{error}");
        assert!(error.contains("evidence"), "{error}");
        assert!(app.store.load_audit_report(&audit.id).unwrap().is_none());
    }
}
