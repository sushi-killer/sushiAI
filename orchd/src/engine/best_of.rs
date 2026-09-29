use super::*;

/// The files and hook registration one implement run needs: its MCP config,
/// its Claude `settings.json` (with the Stop-hook gate registered for
/// `worktree`), and the per-run key file.
pub(super) struct PreparedRun {
    pub mcp_path: PathBuf,
    pub settings_path: PathBuf,
    pub key_path: PathBuf,
    pub messages_server: serde_json::Value,
    pub registered: Option<(String, Arc<HookContext>)>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_run(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    route: &Route,
    settings: &Settings,
    worktree: &Path,
    base_sha: &str,
    attempt_n: u32,
    run_dir: &Path,
    deny_read: &[String],
) -> PreparedRun {
    let mcp_path = run_dir.join("mcp.json");
    let task_mcp_path = app.store.task_dir(task_id).join("mcp.json");
    // The project's own servers, plus orchd's messaging bridge scoped to
    // this task.
    let messages_server = messages::task_server(app, task_id);
    let mut mcp_config = std::fs::read_to_string(&task_mcp_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|v| v.get("mcpServers").is_some_and(|s| s.is_object()))
        .unwrap_or_else(|| json!({"mcpServers": {}}));
    mcp_config["mcpServers"][messages::SERVER] = messages_server.clone();
    let _ = store::write_json_atomic(&mcp_path, &mcp_config);

    let token = uuid::Uuid::new_v4().to_string();
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    // Keep our own clone of the hook context alongside the one handed
    // to `hook.stop` lookups, so we can read back how many times it
    // blocked into `attempt.gateBlocks` once the run ends.
    let mut registered: Option<(String, Arc<HookContext>)> = None;
    if matches!(route.harness, Harness::Claude) {
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
                if store::write_secret_file(&key_path, key).is_ok() {
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

        let socket_path_str = app.socket_path.to_string_lossy().to_string();
        let stop_hook = harness::StopHook {
            orchd_path: &app.orchd_path,
            socket_path: &socket_path_str,
            token: &token,
        };
        let mut claude_settings = harness::build_claude_settings(
            profile_value.as_ref(),
            settings.sandbox,
            &settings.allowed_domains,
            deny_read,
            Some(stop_hook),
        );
        // A headless run answers no prompts, so the messaging tools must
        // be allowed up front to be usable at all.
        if let Some(allow) = claude_settings["permissions"]["allow"].as_array_mut() {
            for tool in crate::mcp::TASK_TOOLS {
                allow.push(json!(format!("mcp__{}__{tool}", messages::SERVER)));
            }
        }
        let _ = store::write_json_atomic(&settings_path, &claude_settings);
        let ctx = Arc::new(HookContext {
            task_id: task_id.to_string(),
            attempt_n,
            repo: task.repo.clone(),
            worktree: worktree.to_path_buf(),
            base_sha: base_sha.to_string(),
            verify: task.verify.clone(),
            blocks: AtomicU32::new(0),
            cancel: CancelToken::new(),
            hook_running: Arc::new(AtomicBool::new(false)),
        });
        app.hook_tokens
            .write()
            .unwrap()
            .insert(token.clone(), ctx.clone());
        registered = Some((token.clone(), ctx));
    }
    PreparedRun {
        mcp_path,
        settings_path,
        key_path,
        messages_server,
        registered,
    }
}

/// What a best-of attempt adds to the task beside the winner's own run.
#[derive(Default)]
pub(super) struct BestOfExtra {
    /// The loser's cost and the pick run's, on top of the winner's.
    pub cost: f64,
    pub candidates: Vec<Candidate>,
    pub decisions: Vec<String>,
    /// The route that produced the winning changes, when it is not the one
    /// the attempt started on.
    pub winner: Option<Route>,
}

/// The second candidate's route when best-of applies to this attempt (the
/// first implement attempt of a hard task with `bestOf` 2), `Err` with a
/// decision line when it applies but there is no route to run it on.
pub(super) fn second_route(
    task: &Task,
    settings: &Settings,
    route: &Route,
    attempt_n: u32,
) -> Result<Option<Route>, String> {
    let variant = task.variant();
    if variant.best_of < 2 || task.tier != Tier::Hard || attempt_n != 1 {
        return Ok(None);
    }
    let other = match variant.best_of_route.as_deref() {
        Some(id) => settings.routes.iter().find(|r| r.id == id),
        None => settings.routes.iter().find(|r| r.harness != route.harness),
    };
    match other {
        Some(r) => Ok(Some(r.clone())),
        None => Err("Best-of: no route on another harness; ran a single candidate".to_string()),
    }
}

struct Eval {
    /// It finished, changed files and every check exited 0.
    passed: bool,
    /// Every plain verify command exited 0 (checks excluded).
    verify_ok: bool,
    /// Failed verify commands and checks; `u32::MAX` when it never finished.
    failing: u32,
    verify: Vec<VerifyOutcome>,
    tally: CheckTally,
}

fn run_finished(res: &Result<harness::RunOutcome, RunError>) -> bool {
    matches!(res, Ok(o) if !o.stalled && !o.over_budget && o.looped.is_none())
}

fn run_cost(app: &App, res: &Result<harness::RunOutcome, RunError>, events: &Path) -> f64 {
    match res {
        Ok(o) => o.cost_usd.unwrap_or(0.0),
        Err(RunError::Cancelled) => replay_run_cost(events, &app.settings.read().unwrap().prices)
            .and_then(|o| o.cost_usd)
            .unwrap_or(0.0),
        Err(RunError::Io(_)) => 0.0,
    }
}

/// A candidate's verify commands, then (with `groundedChecks`) the gated
/// checks and the held-out one; all of them run so the candidates can be
/// compared by how many fail.
#[allow(clippy::too_many_arguments)]
async fn evaluate(
    task: &Task,
    worktree: &Path,
    base_sha: &str,
    dir: &Path,
    finished: bool,
    sandbox: SandboxMode,
    scoped_checks: &[ScopedCheck],
    cancel: &CancelToken,
) -> Eval {
    let (wt, base) = (worktree.to_path_buf(), base_sha.to_string());
    let changed = tokio::task::spawn_blocking(move || {
        let _ = git::stage_all(&wt);
        git::changed_files(&wt, &base).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    if !finished || changed.is_empty() {
        return Eval {
            passed: false,
            verify_ok: false,
            failing: u32::MAX,
            verify: vec![],
            tally: CheckTally::default(),
        };
    }
    // No decision line: this runs once per side, and the winner's own
    // attempt records the skip.
    let (verify_commands, _) = filter_scoped(scoped_checks, &task.repo, &task.verify, &changed);
    let mut verify = run_verify_commands(
        worktree,
        &dir.join("verify"),
        &verify_commands,
        sandbox,
        base_sha,
        cancel,
    )
    .await;
    let plain = verify.len();
    if task.variant().grounded_checks {
        let commands: Vec<String> = gated_checks(task).into_iter().map(|c| c.run).collect();
        if !commands.is_empty() {
            verify.extend(
                run_verify_commands(
                    worktree,
                    &dir.join("checks"),
                    &commands,
                    sandbox,
                    base_sha,
                    cancel,
                )
                .await,
            );
        }
        if let Some(held) = gated_held_out(task) {
            let (result, _) =
                run_held_out(task, &held, worktree, &dir.join("checks"), sandbox, cancel).await;
            verify.push(result);
        }
    }
    let failed = |v: &[VerifyOutcome]| v.iter().filter(|o| o.code != Some(0)).count() as u32;
    let tally = CheckTally {
        passed: (verify.len() - plain) as u32 - failed(&verify[plain..]),
        failed: failed(&verify[plain..]),
    };
    let failing = failed(&verify);
    Eval {
        passed: failing == 0,
        verify_ok: failed(&verify[..plain]) == 0,
        failing,
        verify,
        tally,
    }
}

/// Runs the first implement attempt twice at once: `a_run` in the task's
/// worktree and the same brief on `b_route` in a sibling worktree. The
/// winner's changes end up in the task's worktree, the sibling and its
/// branch are removed, and the winner's run is returned as if it were the
/// only one.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_best_of<F>(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    attempt_n: u32,
    a_run: F,
    a_route: &Route,
    b_route: &Route,
    settings: &Settings,
    worktree: &Path,
    base_sha: &str,
    run_dir: &Path,
    brief_text: &str,
    deny_read: &[String],
    cancel: &CancelToken,
) -> (Result<harness::RunOutcome, RunError>, BestOfExtra)
where
    F: std::future::Future<Output = Result<harness::RunOutcome, RunError>>,
{
    let mut extra = BestOfExtra::default();
    let repo = PathBuf::from(&task.repo);
    let wt_b = PathBuf::from(format!("{}-b", task.worktree));
    let branch_b = format!("{}-b", task.branch);
    let made = {
        let (r, p, b, s) = (
            repo.clone(),
            wt_b.clone(),
            branch_b.clone(),
            base_sha.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            git::create_worktree(&r, &b, &p, &s)?;
            if let Err(e) = git::bootstrap_worktree(&r, &p) {
                git::discard_worktree(&r, &p, &b);
                return Err(git::GitError(e.to_string()));
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(git::GitError(e.to_string())))
    };
    if let Err(e) = made {
        extra.decisions.push(format!(
            "Best-of: could not create the sibling worktree ({e}); ran a single candidate"
        ));
        return (a_run.await, extra);
    }

    let run_dir_b = run_dir.join("b");
    let _ = std::fs::create_dir_all(&run_dir_b);
    let prep = prepare_run(
        app, task, task_id, b_route, settings, &wt_b, base_sha, attempt_n, &run_dir_b, deny_read,
    );
    let _ = std::fs::write(run_dir_b.join("brief.md"), brief_text);
    let events_a = run_dir.join("events.jsonl");
    let events_b = run_dir_b.join("events.jsonl");
    let variant = task.variant();
    let req_b = harness::RunRequest {
        harness: b_route.harness,
        worktree: &wt_b,
        model: b_route.model.as_deref(),
        effort: b_route.effort.as_deref(),
        max_budget_usd: (variant.max_attempt_cost_usd > 0.0)
            .then_some(variant.max_attempt_cost_usd),
        review: false,
        mcp_config: Some(&prep.mcp_path),
        settings_path: Some(&prep.settings_path),
        network_allowed: settings.codex_network,
        codex_mcp: Some((messages::SERVER, &prep.messages_server)),
        images: &[],
        repo_settings: true,
    };
    let stall_b = (variant.stall_timeout_secs > 0).then(|| Stall {
        limit: Duration::from_secs(variant.stall_timeout_secs),
        paused: prep
            .registered
            .as_ref()
            .map(|(_, ctx)| ctx.hook_running.clone())
            .unwrap_or_default(),
    });
    let b_run = run_harness(
        app,
        task_id,
        attempt_n,
        false,
        &wt_b,
        &req_b,
        CostTag::task("implement", &b_route.id),
        brief_text,
        &events_b,
        cancel,
        stall_b,
        variant.loop_detect.then(LoopDetector::new),
    );
    let (a_res, b_res) = tokio::join!(a_run, b_run);
    if let Some((token, ctx)) = &prep.registered {
        app.hook_tokens.write().unwrap().remove(token);
        ctx.cancel.cancel();
    }
    let _ = std::fs::remove_file(&prep.key_path);

    let cost_a = run_cost(app, &a_res, &events_a);
    let cost_b = run_cost(app, &b_res, &events_b);
    let cleanup = |wt: PathBuf, branch: String, repo: PathBuf| async move {
        let _ =
            tokio::task::spawn_blocking(move || git::discard_worktree(&repo, &wt, &branch)).await;
    };
    if matches!(a_res, Err(RunError::Cancelled)) || matches!(b_res, Err(RunError::Cancelled)) {
        extra.cost = cost_b;
        cleanup(wt_b, branch_b, repo).await;
        return (Err(RunError::Cancelled), extra);
    }

    let sandbox = settings.sandbox;
    let (eval_a, eval_b) = tokio::join!(
        evaluate(
            task,
            worktree,
            base_sha,
            run_dir,
            run_finished(&a_res),
            sandbox,
            &settings.scoped_checks,
            cancel
        ),
        evaluate(
            task,
            &wt_b,
            base_sha,
            &run_dir_b,
            run_finished(&b_res),
            sandbox,
            &settings.scoped_checks,
            cancel
        ),
    );

    let (id_a, id_b) = (a_route.id.as_str(), b_route.id.as_str());
    let mut pick_b;
    let decision;
    match (eval_a.passed, eval_b.passed) {
        (true, true) => {
            match run_pick(
                app, task, task_id, attempt_n, settings, a_route, b_route, worktree, &wt_b,
                base_sha, run_dir, &eval_a, &eval_b, deny_read, cancel,
            )
            .await
            {
                Ok((Some((pick, why)), cost)) => {
                    extra.cost += cost;
                    pick_b = pick;
                    decision = format!(
                        "Best-of: both passed; picked {} ({}) - {why}",
                        if pick_b { "b" } else { "a" },
                        if pick_b { id_b } else { id_a }
                    );
                }
                Ok((None, cost)) => {
                    extra.cost += cost;
                    pick_b = false;
                    decision = format!(
                        "Best-of: both passed; the pick run gave no answer, kept a ({id_a})"
                    );
                }
                Err(_) => {
                    extra.cost += cost_b;
                    cleanup(wt_b, branch_b, repo).await;
                    return (Err(RunError::Cancelled), extra);
                }
            }
        }
        (true, false) => {
            pick_b = false;
            decision = format!("Best-of: only a ({id_a}) passed verify and the checks");
        }
        (false, true) => {
            pick_b = true;
            decision = format!("Best-of: only b ({id_b}) passed verify and the checks");
        }
        (false, false) => {
            pick_b = eval_b.failing < eval_a.failing;
            let fewer = |n: u32| {
                if n == u32::MAX {
                    "unfinished".to_string()
                } else {
                    format!("{n} failing")
                }
            };
            decision = format!(
                "Best-of: neither passed; continuing from {} ({} vs {})",
                if pick_b {
                    format!("b ({id_b})")
                } else {
                    format!("a ({id_a})")
                },
                fewer(if pick_b {
                    eval_b.failing
                } else {
                    eval_a.failing
                }),
                fewer(if pick_b {
                    eval_a.failing
                } else {
                    eval_b.failing
                }),
            );
        }
    }
    let mut decision = decision;

    if pick_b {
        // Carry the winner into the task's worktree; if that fails the task
        // worktree still holds candidate a, put back as it was.
        let (wa, wb, base) = (worktree.to_path_buf(), wt_b.clone(), base_sha.to_string());
        let carried = tokio::task::spawn_blocking(move || {
            let mine = git::patch_since(&wa, &base)?;
            let theirs = git::patch_since(&wb, &base)?;
            if let Err(e) = git::replace_work_with_patch(&wa, &base, &theirs) {
                let _ = git::replace_work_with_patch(&wa, &base, &mine);
                return Err(e);
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
        if let Err(e) = carried {
            pick_b = false;
            decision.push_str(&format!(
                "; could not carry b into the task worktree ({e}), kept a"
            ));
        }
    }
    cleanup(wt_b, branch_b, repo).await;

    extra.decisions.push(decision);
    extra.candidates = [
        (a_route, cost_a, &eval_a, !pick_b),
        (b_route, cost_b, &eval_b, pick_b),
    ]
    .into_iter()
    .map(|(route, cost, eval, picked)| Candidate {
        route: route.id.clone(),
        cost,
        verify: eval.verify_ok,
        checks: eval.tally,
        picked,
    })
    .collect();
    if pick_b {
        extra.cost += cost_a;
        extra.winner = Some(b_route.clone());
        (b_res, extra)
    } else {
        extra.cost += cost_b;
        (a_res, extra)
    }
}

/// One read-only run that reads both diffs and answers with a `sushi-pick`
/// block; `Some((b_wins, why))`, or `None` when the reply had none.
#[allow(clippy::too_many_arguments)]
async fn run_pick(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    attempt_n: u32,
    settings: &Settings,
    a_route: &Route,
    b_route: &Route,
    worktree: &Path,
    wt_b: &Path,
    base_sha: &str,
    run_dir: &Path,
    eval_a: &Eval,
    eval_b: &Eval,
    deny_read: &[String],
    cancel: &CancelToken,
) -> Result<(Option<(bool, String)>, f64), RunError> {
    // The reviewer on the other family; without a review route, whichever
    // family candidate a was not.
    let route = select_review_route(settings, a_route, task.tier)
        .map(|(r, _)| r)
        .filter(|r| r.harness != a_route.harness)
        .unwrap_or(b_route)
        .clone();
    let (wa, wb, base) = (
        worktree.to_path_buf(),
        wt_b.to_path_buf(),
        base_sha.to_string(),
    );
    let (diff_a, diff_b) = tokio::task::spawn_blocking(move || {
        (
            git::diff_full(&wa, &base, 30_000).unwrap_or_default(),
            git::diff_full(&wb, &base, 30_000).unwrap_or_default(),
        )
    })
    .await
    .unwrap_or_default();
    let brief_text = brief::build_pick_brief(
        task,
        &brief::PickCandidate {
            verify: &eval_a.verify,
            diff: &diff_a,
        },
        &brief::PickCandidate {
            verify: &eval_b.verify,
            diff: &diff_b,
        },
    );
    let dir = run_dir.join("pick");
    let _ = std::fs::create_dir_all(&dir);
    let mcp_path = dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(dir.join("brief.md"), &brief_text);
    let settings_path = dir.join("settings.json");
    let key_path = dir.join("key");
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            settings,
            deny_read,
            &settings_path,
        );
    }
    let events = dir.join("events.jsonl");
    let req = review_request(&route, worktree, &mcp_path, &settings_path, &[]);
    let res = run_harness(
        app,
        task_id,
        attempt_n,
        false,
        worktree,
        &req,
        CostTag::task("pick", &route.id),
        &brief_text,
        &events,
        cancel,
        None,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    match res {
        Ok(outcome) => {
            let cost = outcome.cost_usd.unwrap_or(0.0);
            let text = outcome.final_text.unwrap_or_default();
            Ok((brief::parse_pick(&text), cost))
        }
        Err(RunError::Cancelled) => Err(RunError::Cancelled),
        Err(RunError::Io(_)) => Ok((None, 0.0)),
    }
}
