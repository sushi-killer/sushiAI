use super::*;

pub(super) enum RunError {
    Cancelled,
    Io(String),
}

/// Spawn `claude`/`codex` in its own process group, write `brief` to
/// stdin, stream stdout into `events.jsonl` and the log broadcast, and wait
/// for it to exit. `cancel` triggers SIGTERM to the group, then SIGKILL
/// after 5s (spec: "Each child in its own process group; stop = SIGTERM to
/// the group, SIGKILL after 5s").
///
/// `track_attempt` gates the mid-run `pgid`/`session_id` persistence below:
/// `true` for a session that *is* `task.attempts[_]` with number `attempt_n`
/// (the implement or plan attempt itself); `false` for a nested session
/// that merely borrows that attempt's number for its run directory (review,
/// orchestrator triage) -- otherwise its own pgid/session id would
/// overwrite the real attempt's, leaving it stuck `running` with a session
/// id `--resume` should never see and a pgid recovery would `killpg` on a
/// process that already exited (P1, triage review). A nested session's own
/// child is still reachable through `cancel` for as long as this call is
/// awaited, so skipping persistence here only affects recovery after a
/// daemon crash mid-review/-triage, not the normal stop path.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_harness(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track_attempt: bool,
    worktree: &Path,
    req: &harness::RunRequest<'_>,
    brief_text: &str,
    events_path: &Path,
    cancel: &CancelToken,
    stall: Option<Stall>,
) -> Result<harness::RunOutcome, RunError> {
    let argv = harness::build_argv(req);
    let bin = resolve_binary(req.harness);
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&argv)
        .current_dir(worktree)
        .env("PATH", augmented_path())
        // Lets a repo's own hooks tell an orchd-run agent from a person's
        // session (sushiAI's lesson reminder stays quiet for it).
        .env("ORCHD_TASK", task_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::Io(format!("{bin}: {e}")))?;
    let pgid = child.id().map(|p| p as i32);
    if track_attempt {
        if let Some(pgid) = pgid {
            persist_attempt_field(app, task_id, attempt_n, move |a| a.pgid = Some(pgid)).await;
        }
    }

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(brief_text.as_bytes()).await;
        drop(stdin);
    }

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut out_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));

    let mut outcome = harness::RunOutcome::default();
    let harness_kind = req.harness;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stderr_tail = String::new();
    let mut session_persisted = false;
    // ponytail: silence is the only stall signal; a long Bash call or the
    // Stop hook's verify (up to 540s) is silent too, so the timeout must sit
    // above them.
    // select! builds a disabled branch's future too: an unset timeout still
    // needs a deadline that doesn't overflow `Instant`.
    let stall_limit = stall
        .as_ref()
        .map_or(Duration::from_secs(365 * 24 * 3600), |s| s.limit);
    let mut last_output = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                kill_group(pgid, &mut child).await;
                return Err(RunError::Cancelled);
            }
            _ = tokio::time::sleep_until(last_output + stall_limit), if stall.is_some() => {
                if stall.as_ref().is_some_and(|s| s.paused.load(Ordering::SeqCst)) {
                    last_output = tokio::time::Instant::now();
                    continue;
                }
                kill_group(pgid, &mut child).await;
                outcome.stalled = true;
                outcome.error = Some(format!(
                    "No output for {}s; the run was stopped as stalled.",
                    stall_limit.as_secs()
                ));
                break;
            }
            line = out_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(l)) => {
                        last_output = tokio::time::Instant::now();
                        append_line(events_path, &l);
                        if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                            app.broadcast_log(task_id, attempt_n, note);
                        }
                        if track_attempt && !session_persisted {
                            if let Some(sid) = outcome.session_id.clone() {
                                session_persisted = true;
                                persist_attempt_field(app, task_id, attempt_n, move |a| a.session_id = Some(sid)).await;
                            }
                        }
                    }
                    _ => { stdout_done = true; }
                }
            }
            line = err_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(l)) => {
                        last_output = tokio::time::Instant::now();
                        append_line(events_path, &format!("[stderr] {l}"));
                        stderr_tail.push_str(&l);
                        stderr_tail.push('\n');
                        if stderr_tail.len() > 4000 {
                            let cut = stderr_tail.len() - 4000;
                            let cut = (cut..stderr_tail.len()).find(|i| stderr_tail.is_char_boundary(*i)).unwrap_or(0);
                            stderr_tail.drain(..cut);
                        }
                    }
                    _ => { stderr_done = true; }
                }
            }
            status = child.wait(), if stdout_done && stderr_done => {
                if let Ok(status) = status {
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            format!("{:?} exited with {status}.", req.harness)
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    if outcome.cost_usd.is_none() && req.harness == Harness::Codex {
        let price = req
            .model
            .and_then(|m| app.settings.read().unwrap().prices.get(m).copied());
        outcome.cost_usd = price.map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        });
    }
    if req.harness == Harness::Claude {
        outcome.estimate_cost(&app.settings.read().unwrap().prices);
    }
    Ok(outcome)
}
