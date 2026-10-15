use super::turn::{ExecutedTurn, Turn};
use crate::clock::now_ms;
use crate::engine::owner_notice;
use crate::engine::state::App;
use crate::harness::native::error::RunError;
use crate::overseer;
use crate::overseer_state;

/// How an overseer turn ended, for the log.
pub(in crate::engine) enum TurnEnd {
    Reply(String),
    Stopped,
    Failed(String),
    Silent,
}

pub(in crate::engine) struct OverseerRecord<'a> {
    pub(in crate::engine) reason: &'a str,
    pub(in crate::engine) task_id: Option<&'a str>,
    pub(in crate::engine) started: i64,
    pub(in crate::engine) cost_usd: Option<f64>,
    pub(in crate::engine) end: TurnEnd,
    pub(in crate::engine) gate: Option<owner_notice::Gate>,
}

/// Logs how an overseer turn ended: stopped, failed, the digest's first line,
/// what the agent left for the owner when it did not answer or open a pull
/// request for the task itself, or a bare `done` that still carries the cost.
pub(in crate::engine) fn record_overseer_end(app: &App, repo: &str, record: OverseerRecord<'_>) {
    let OverseerRecord {
        reason,
        task_id,
        started,
        cost_usd,
        end,
        gate,
    } = record;
    use owner_notice::Verdict;
    let acted = || {
        overseer::read(&app.data_dir, Some(repo), None)
            .iter()
            .any(|e| {
                e.ts >= started
                    && matches!(e.kind.as_str(), "answered" | "pr")
                    && e.task_id.as_deref() == task_id
            })
    };
    // What the entry says, and the full reply behind it when that differs.
    let verdict = gate.as_ref().map(|g| g.verdict);
    let (kind, text, detail) = match end {
        TurnEnd::Stopped => ("skipped", "Stopped by you".to_string(), None),
        TurnEnd::Failed(error) => ("failed", error, None),
        TurnEnd::Reply(reply) if reason == "digest" => ("digest", reply, None),
        TurnEnd::Reply(reply) => match verdict {
            Some(Verdict::Notify) => ("asked_owner", reply, None),
            Some(Verdict::Duplicate) => {
                ("skipped", owner_notice::DUPLICATE.to_string(), Some(reply))
            }
            Some(Verdict::Deferred) => ("skipped", owner_notice::DEFERRED.to_string(), Some(reply)),
            // The turn settled its questions itself: its words stay in the detail.
            Some(Verdict::Resolved) => ("done", reply, None),
            None if !acted() => ("asked_owner", reply, None),
            None => ("done", reply, None),
        },
        TurnEnd::Silent if cost_usd.is_some() => ("done", "Finished".to_string(), None),
        TurnEnd::Silent => return,
    };
    let mut entry = overseer::Entry::new(kind, repo, task_id, &text)
        .with_detail(detail.as_deref().unwrap_or(&text));
    // Only a notice (or the held one) remembers its decisions.
    let tells = matches!(kind, "asked_owner" | "digest")
        || (kind == "skipped" && verdict == Some(Verdict::Deferred));
    if let Some(gate) = gate.filter(|_| tells) {
        entry = entry.with_decision(&gate.keys);
    }
    entry.cost_usd = cost_usd;
    app.record_overseer(entry);
}

pub(super) struct NoticeDisposition {
    pub(super) blocked: bool,
    pub(super) notice: Option<String>,
}

pub(super) fn finish_overseer(
    app: &App,
    repo: &str,
    turn: &Turn,
    execution: &ExecutedTurn,
) -> NoticeDisposition {
    let mut blocked = false;
    let mut notice: Option<String> = None;
    if !matches!(turn, Turn::Owner(_)) {
        let timed = matches!(execution.result, Err(RunError::Cancelled)) && execution.timed_out;
        let (cost, mut end) = match &execution.result {
            Ok(o) => (
                o.cost_usd,
                match (
                    o.final_text.as_ref().filter(|t| !t.trim().is_empty()),
                    &o.error,
                ) {
                    (Some(reply), _) => TurnEnd::Reply(reply.clone()),
                    (None, Some(error)) => TurnEnd::Failed(error.clone()),
                    (None, None) => TurnEnd::Silent,
                },
            ),
            Err(RunError::Cancelled) if timed => (None, TurnEnd::Failed("timed out".to_string())),
            Err(RunError::Cancelled) => (None, TurnEnd::Stopped),
            Err(RunError::Io(error) | RunError::NotFound(error)) => {
                (None, TurnEnd::Failed(error.clone()))
            }
        };
        let (reason, task_id) = match turn {
            Turn::Overseer {
                reason, task_id, ..
            } => (*reason, task_id.as_deref()),
            _ => ("questions", None),
        };
        let outcome = match &end {
            TurnEnd::Reply(reply) => reply.clone(),
            TurnEnd::Stopped => "stopped".to_string(),
            TurnEnd::Failed(_) if timed => "timed out".to_string(),
            TurnEnd::Failed(error) => format!("failed: {error}"),
            TurnEnd::Silent => "no reply".to_string(),
        };
        overseer_state::add_decision(&app.data_dir, repo, now_ms(), reason, task_id, &outcome);
        if let Turn::Overseer { held, about, .. } = turn {
            let gate = match &mut end {
                TurnEnd::Reply(reply) => {
                    let gate = app.overseer_gate(repo, reason, about);
                    if gate.is_some() && reason != "digest" {
                        *reply = owner_notice::cap_notice(reply);
                        notice = Some(reply.clone());
                    }
                    blocked = gate
                        .as_ref()
                        .is_some_and(|g| g.verdict != owner_notice::Verdict::Notify);
                    gate
                }
                _ => None,
            };
            // Still waiting after the turn: it escalated, the owner is asked
            // now, unless quiet hours hold the notice (handed over when they end).
            let hand_over = gate
                .as_ref()
                .is_none_or(|g| g.verdict != owner_notice::Verdict::Deferred);
            record_overseer_end(
                app,
                repo,
                OverseerRecord {
                    reason,
                    task_id,
                    started: execution.started,
                    cost_usd: cost,
                    end,
                    gate,
                },
            );
            app.release_holds(repo, held, hand_over);
        }
    }
    NoticeDisposition { blocked, notice }
}
