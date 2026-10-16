use super::store::{save, turn_dir};
use super::tools::{keep_tools, tools_in_line};
use super::types::ChatTool;
use crate::clip::tail_chars;
use crate::clock::now_ms;
use crate::engine::attempts::ledger::record_run;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::pricing::finalize_cost;
use crate::engine::attempts::run::RunIdentity;
use crate::engine::chat_tools;
use crate::engine::state::App;
use crate::harness;
use crate::harness::environment::augmented_path;
use crate::harness::environment::resolve_binary;
use crate::harness::native::error::RunError;
use crate::store::log::append_line;
use std::sync::Arc;

use super::launch::PreparedLaunch;
use super::turn::ScheduledTurn;

pub(super) async fn run(
    app: &Arc<App>,
    scheduled: &mut ScheduledTurn,
    launch: &PreparedLaunch,
) -> Result<(harness::RunOutcome, Vec<ChatTool>), RunError> {
    let started_at = now_ms();
    let mut child = spawn(scheduled, launch).await?;
    let pgid = child.id().map(|p| p as i32);
    let repo = scheduled.repo.as_str();
    let harness_kind = scheduled.route.harness;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut out = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));
    let events = turn_dir(app, repo).join("events.jsonl");
    let mut outcome = harness::RunOutcome::default();
    let mut stderr_tail = String::new();
    let mut tools = Vec::new();
    let mut seen_items = std::collections::HashSet::new();
    let (mut out_done, mut err_done) = (false, false);
    loop {
        tokio::select! {
            _ = scheduled.cancel.cancelled() => {
                crate::procs::kill_group(pgid, &mut child).await;
                record_cost(app, scheduled, launch, started_at, &mut outcome.clone());
                return Err(RunError::Cancelled);
            }
            line = out.next_line(), if !out_done => match line {
                Ok(Some(l)) => {
                    append_line(&events, &l);
                    chat_tools::discovery::note_init(app, &l);
                    keep_tools(
                        &mut tools,
                        tools_in_line(app, repo, harness_kind, &l, &mut seen_items),
                    );
                    if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                        scheduled.thread.note = Some(note);
                        save(app, repo, &scheduled.thread);
                    }
                }
                _ => out_done = true,
            },
            line = err.next_line(), if !err_done => match line {
                Ok(Some(l)) => {
                    stderr_tail = tail_chars(&format!("{stderr_tail}{l}\n"), 2000);
                }
                _ => err_done = true,
            },
            status = child.wait(), if out_done && err_done => {
                if let Ok(status) = status {
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            crate::harness::exit_error(harness_kind, status)
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    record_cost(app, scheduled, launch, started_at, &mut outcome);
    Ok((outcome, tools))
}

async fn spawn(
    scheduled: &ScheduledTurn,
    launch: &PreparedLaunch,
) -> Result<tokio::process::Child, RunError> {
    let bin = resolve_binary(scheduled.route.harness);
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&launch.argv)
        .current_dir(&scheduled.repo)
        .env("PATH", augmented_path())
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
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(scheduled.text.as_bytes()).await;
    }
    Ok(child)
}

fn record_cost(
    app: &App,
    scheduled: &ScheduledTurn,
    launch: &PreparedLaunch,
    started_at: i64,
    outcome: &mut harness::RunOutcome,
) {
    let route = &scheduled.route;
    let repo = &scheduled.repo;
    let harness_kind = route.harness;
    finalize_cost(app, harness_kind, route.model.as_deref(), outcome);
    outcome.fingerprint = Some(outcome.build_fingerprint(harness_kind, None, String::new()));
    record_run(
        app,
        &RunIdentity {
            id: "",
            attempt_n: 0,
            track: crate::engine::attempts::tracking::RunTrack::No,
            tag: CostTag::repo("chat", &route.id, repo),
        },
        crate::engine::attempts::pricing::ProviderRun {
            harness: harness_kind,
            model: route.model.as_deref(),
            resume: scheduled
                .thread
                .session_id
                .as_deref()
                .filter(|_| launch.resumed),
        },
        outcome,
        started_at,
    );
}
