use crate::clock::now_ms;
use crate::engine::advisor::run_advisor_before_retry;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::attempts::best_of::run::run_best_of;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::prepare::prepare_run;
use crate::engine::attempts::prepare::PreparedRun;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::availability::wait_for_harness;
use crate::engine::cost::wait_while_over_budget;
use crate::engine::decision::implement_attempt_count;
use crate::engine::decision::last_review_repeated;
use crate::engine::ladder;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::lifecycle::resume::resume_attempt;
use crate::engine::run_control::HookContext;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::healing::answers::{heal_answers, AnswerHealing};
use crate::harness;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::best_of::second_route;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::store;
use crate::task::FailureKind;
use crate::task::Task;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(in crate::engine) struct ParentFailure<'a> {
    pub(in crate::engine) detail: &'a str,
    pub(in crate::engine) conflict_only: bool,
}

#[derive(Clone, Copy)]
pub(in crate::engine) enum ExecutionMode<'a> {
    Adopt(usize),
    Start(Option<ParentFailure<'a>>),
}

pub(in crate::engine) struct RunReceipt {
    pub(in crate::engine) settings: Settings,
    pub(in crate::engine) attempt_n: u32,
    pub(in crate::engine) idx: usize,
    pub(in crate::engine) route: Route,
    pub(in crate::engine) worktree: PathBuf,
    pub(in crate::engine) base_sha: String,
    pub(in crate::engine) run_dir: PathBuf,
}

pub(in crate::engine) struct ExecutedAttempt {
    pub(in crate::engine) run: RunReceipt,
    pub(in crate::engine) key_path: PathBuf,
    pub(in crate::engine) registered: Option<(String, Arc<HookContext>)>,
    pub(in crate::engine) run_result: Result<harness::RunOutcome, RunError>,
    pub(in crate::engine) best_of: BestOfExtra,
}

pub(in crate::engine) enum ExecutionStep {
    Executed(Box<ExecutedAttempt>),
    Retry,
    Stop,
}

pub(in crate::engine) async fn execute(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
    mode: ExecutionMode<'_>,
) -> ExecutionStep {
    let task_id = task.id.clone();
    let pending_answer = signals.pending_answer.clone();
    let cancel = signals.cancel.clone();
    let execution = if let ExecutionMode::Adopt(idx) = mode {
        crate::engine::lifecycle::adopted::follow(app, task, idx, &cancel).await
    } else {
        let ExecutionMode::Start(parent) = mode else {
            unreachable!()
        };
        let parent_conflict_only = parent.as_ref().is_some_and(|cause| cause.conflict_only);
        let parent_failure = parent.as_ref().map(|cause| cause.detail);
        let resume_status = task.status;
        if !wait_for_harness(
            app,
            &task_id,
            task,
            resume_status,
            &pending_answer,
            &cancel,
            permit,
        )
        .await
        {
            *permit = None;
            app.finish_task_loop(&task_id);
            return ExecutionStep::Stop;
        }
        let settings = app.settings.read().unwrap().clone();
        let attempt_n = crate::task::attempts::next_number(task);
        let Some(selected) =
            crate::engine::lifecycle::selection::route(app, task, &settings, parent_conflict_only)
        else {
            return ExecutionStep::Retry;
        };
        let crate::engine::lifecycle::selection::Selection {
            route,
            conflict_only,
            carry_conflicts,
        } = &selected;
        let route = route.clone();
        let conflict_only = *conflict_only;
        let carry_conflicts = carry_conflicts.clone();

        let worktree = PathBuf::from(&task.worktree);
        let base_sha = task.base_sha.clone();
        let wt2 = worktree.clone();
        let base2 = base_sha.clone();
        let (status_short, diff_stat) = tokio::task::spawn_blocking(move || {
            (
                crate::git::changes::status_short(&wt2).unwrap_or_default(),
                crate::git::changes::diff_stat(&wt2, &base2).unwrap_or_default(),
            )
        })
        .await
        .unwrap_or_default();

        // A finding the reviewer says came back gets the advisor whether or
        // not the variant asks for it.
        let repeated_finding = last_review_repeated(task) || ladder::approach_pending(task);
        if task.variant().advisor || repeated_finding {
            if let Some(cost) = run_advisor_before_retry(
                app,
                task,
                &settings,
                &route,
                &worktree,
                &base_sha,
                repeated_finding,
                &cancel,
            )
            .await
            {
                task.charge(cost);
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                // The advisor's spend can be what reaches the budget.
                let status = task.status;
                if !wait_while_over_budget(
                    app,
                    &task_id,
                    task,
                    "implement",
                    status,
                    &pending_answer,
                    &cancel,
                    permit,
                )
                .await
                {
                    *permit = None;
                    app.finish_task_loop(&task_id);
                    return ExecutionStep::Stop;
                }
            }
        }

        // An owner or policy answer that contradicts a criterion amends it
        // before the attempt is briefed.
        match heal_answers(app, task, attempt_n, &cancel).await {
            Ok(AnswerHealing::Ready) => {}
            Ok(AnswerHealing::Retry) => return ExecutionStep::Retry,
            Ok(AnswerHealing::Stopped) | Err(_) => {
                *permit = None;
                mark_stopped_if_not_already(app, &task_id).await;
                app.finish_task_loop(&task_id);
                return ExecutionStep::Stop;
            }
        }
        let crate::engine::lifecycle::implementation_brief::FullBrief {
            text: brief_text,
            messages,
            advice,
        } = crate::engine::lifecycle::implementation_brief::build(
            app,
            task,
            &selected,
            parent_failure,
            (&status_short, &diff_stat),
        );

        let tools = crate::engine::permissions::tools::task_tools(app, &settings, task, &worktree);
        for line in &tools.notes {
            if !task.decisions.contains(line) {
                task.record_decision(line.clone());
            }
        }
        let brief_text = crate::engine::attempts::context::with_block_before_report(
            &brief_text,
            &crate::engine::permissions::brief::tools_block(&tools, false),
        );
        let mut no_second = BestOfExtra::default();
        // Recorded after the reload below, which would drop a line pushed now.
        let second = match if conflict_only {
            Ok(None)
        } else {
            second_route(task, &settings, &route, implement_attempt_count(task) == 0)
        } {
            Ok(second) => second,
            Err(line) => {
                no_second.decisions.push(line);
                None
            }
        };
        let full_brief = brief_text;
        let previous = resume_attempt(task, &route, conflict_only, second.is_some());
        let resume = previous.and_then(|a| a.session_id.clone());
        let brief_text = match previous {
            Some(a) => crate::engine::attempts::resume::build_resume_brief(
                task,
                a,
                &messages,
                advice.as_deref(),
            ),
            None => full_brief.clone(),
        };
        let resume_reason = if previous
            .and_then(|a| a.failure.as_ref())
            .is_some_and(|f| f.kind == FailureKind::TimeBudget)
        {
            "its time budget"
        } else {
            "a failed check"
        };
        if let Some(session) = &resume {
            task.record_decision(format!(
                    "Orchestrator: attempt {attempt_n} resumes session {session} on route {} after {resume_reason}",
                    route.id
                ));
        }

        let reason = if conflict_only {
            format!("conflict only -> route {}", route.id)
        } else {
            format!("tier {} -> route {}", task.tier.as_str(), route.id)
        };
        let idx = match crate::engine::lifecycle::attempt_start::start(
            app,
            task,
            &route,
            &reason,
            &carry_conflicts,
        ) {
            Ok(Some(index)) => index,
            Ok(None) => return ExecutionStep::Retry,
            Err(error) => {
                eprintln!("orch: starting attempt for {task_id} failed: {error}");
                *permit = None;
                app.finish_task_loop(&task_id);
                return ExecutionStep::Stop;
            }
        };

        let run_dir = app.store.run_dir(&task_id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        let PreparedRun {
            mcp_path,
            settings_path,
            key_path,
            messages_server,
            registered,
            sandbox,
        } = prepare_run(
            app, task, &task_id, &route, &settings, &worktree, attempt_n, &run_dir, &tools,
        );

        let network_allowed = settings.codex_network;
        let codex_mcp = tools.codex_servers((sandbox.is_none()).then_some(&messages_server));
        let mut req = harness::RunRequest {
            harness: route.harness,
            worktree: &worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            max_budget_usd: (task.variant().max_attempt_cost_usd > 0.0)
                .then(|| task.variant().max_attempt_cost_usd),
            review: false,
            task_run: true,
            task_sandbox: sandbox.as_ref(),
            mcp_config: Some(&mcp_path),
            settings_path: Some(&settings_path),
            network_allowed,
            codex_mcp: &codex_mcp,
            images: &[],
            repo_settings: true,
            resume: resume.as_deref(),
        };

        let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
        let events_path = run_dir.join("events.jsonl");
        let a_run = run_harness(
            app,
            &task_id,
            attempt_n,
            RunTrack::Resumable,
            &worktree,
            &req,
            CostTag::task("implement", &route.id),
            &brief_text,
            &events_path,
            &cancel,
            Guard::for_variant(
                &task.variant(),
                registered.as_ref().map(|(_, ctx)| ctx.hook_running.clone()),
            )
            .with_time_budget(
                settings.implement_budget_mins,
                task.attempts[idx].started_at,
            ),
        );
        let (mut run_result, mut best_of) = match second {
            Some(b_route) => {
                run_best_of(
                    app,
                    task,
                    &task_id,
                    attempt_n,
                    a_run,
                    &route,
                    &b_route,
                    &settings,
                    &worktree,
                    &base_sha,
                    &run_dir,
                    &brief_text,
                    &deny_read,
                    &cancel,
                )
                .await
            }
            None => (a_run.await, no_second),
        };
        let rejected = match &run_result {
            Ok(outcome) => outcome.error.is_some() && outcome.session_id.is_none(),
            Err(RunError::Io(_) | RunError::NotFound(_)) => true,
            Err(RunError::Cancelled) => false,
        };
        if req.resume.is_some() && rejected && !cancel.is_cancelled() {
            best_of.decisions.push(format!(
                    "Orchestrator: attempt {attempt_n} resume rejected without a session init; falling back to a fresh run in the same attempt"
                ));
            req.resume = None;
            let _ = std::fs::rename(&events_path, run_dir.join("resume-rejected.events.jsonl"));
            let _ = std::fs::write(run_dir.join("brief.md"), &full_brief);
            // Subscription credentials are consumed when the CLI starts.
            if let Some(account) = route
                .account_id
                .as_ref()
                .and_then(|id| app.secrets.read().unwrap().accounts.get(id).cloned())
            {
                if account.kind == "subscription" {
                    let _ = store::files::write_secret_file(
                        &run_dir.join("oauth-token"),
                        &account.value,
                    );
                }
            }
            run_result = run_harness(
                app,
                &task_id,
                attempt_n,
                RunTrack::Resumable,
                &worktree,
                &req,
                CostTag::task("implement", &route.id),
                &full_brief,
                &events_path,
                &cancel,
                Guard::for_variant(
                    &task.variant(),
                    registered.as_ref().map(|(_, ctx)| ctx.hook_running.clone()),
                )
                .with_time_budget(
                    settings.implement_budget_mins,
                    task.attempts[idx].started_at,
                ),
            )
            .await;
        }
        ExecutedAttempt {
            run: RunReceipt {
                settings,
                attempt_n,
                idx,
                route,
                worktree,
                base_sha,
                run_dir,
            },
            key_path,
            registered,
            run_result,
            best_of,
        }
    };
    ExecutionStep::Executed(Box::new(execution))
}
