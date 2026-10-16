use super::judge::{Judge, JudgePurpose, JudgeRun};
use super::reply::{refresh_brief, reply_list};
use super::routing::check_route_for;
use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::answer_policy::authority::{check_authority, shared_check_amend_refusal};
use crate::engine::decision::completed_implement_attempts;
use crate::engine::persistence::contract::ContractInput;
use crate::engine::planning::graph::overlapping_path;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::task::amendment::Amendment;
use crate::task::checks::authority::Origin;
use crate::task::checks::commands::check_targets;
use crate::task::{AttemptStatus, Stage, Task};
use std::path::PathBuf;
use std::sync::Arc;

/// The files the dependencies changed: each one's landed diff (or, not yet
/// landed, its branch over its base) read from git; the last passed implement
/// attempt's changed files when git cannot read the range.
async fn landed_files(app: &App, task: &Task) -> Vec<String> {
    let deps: Vec<Task> = task
        .depends_on
        .iter()
        .filter_map(|id| app.store.load_task(id).ok().flatten())
        .collect();
    let mut files: Vec<String> = Vec::new();
    for dep in deps {
        let (from, to) = crate::engine::reports::task_range(&dep);
        let repo = PathBuf::from(&dep.repo);
        let from_git = tokio::task::spawn_blocking(move || {
            crate::git::changes::numstat(&repo, &from, &to).ok()
        })
        .await
        .unwrap_or(None);
        let paths: Vec<String> = match from_git {
            Some(entries) => entries.into_iter().map(|e| e.0).collect(),
            None => dep
                .attempts
                .iter()
                .rev()
                .find(|a| a.stage == Stage::Implement && a.status == AttemptStatus::Passed)
                .map(|a| a.changed_files.clone())
                .unwrap_or_default(),
        };
        for f in paths {
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    files
}

struct RefreshEdit {
    amendment: Amendment,
    paths: Option<Vec<String>>,
}

struct RefreshRuling {
    decision: Option<String>,
    edit: Option<RefreshEdit>,
}

fn settle_refresh(
    app: &Arc<App>,
    task: &Task,
    input: &ContractInput,
    ruling: RefreshRuling,
    cancel: &CancelToken,
) -> Result<(), WaitEnd> {
    let changed = ruling
        .edit
        .as_ref()
        .is_some_and(|edit| edit.amendment.verify.is_some());
    let publication = input
        .publish(app, task, cancel, |fresh| {
            if let Some(edit) = ruling.edit {
                let authority = check_authority(&app.settings.read().unwrap(), fresh);
                edit.amendment
                    .apply_authorized(fresh, Origin::Other, &authority, now_ms())?;
                if let Some(paths) = edit.paths {
                    fresh.record_paths(paths);
                }
            }
            fresh.mark_dependency_refresh();
            fresh.record_decisions(ruling.decision);
            Ok(())
        })
        .map_err(|error| {
            eprintln!("orch: dependency refresh publication {}: {error}", task.id);
            WaitEnd::Stopped
        })?;
    if let Err(end) = publication.into_result() {
        app.append_decision(
            &task.id,
            "Refresh: deferred the result after the task changed".into(),
        );
        return Err(end);
    }
    if changed {
        app.verify_cache.lock().unwrap().remove(&task.id);
    }
    Ok(())
}

/// Refreshes a parent-drafted child's commands after its dependencies land.
pub(in crate::engine) async fn refresh_after_dependencies(
    app: &Arc<App>,
    task_id: &str,
    cancel: &CancelToken,
) -> Result<(), WaitEnd> {
    let task = app
        .store
        .load_task(task_id)
        .map_err(|error| {
            eprintln!("orch: dependency refresh {task_id}: {error}");
            WaitEnd::Stopped
        })?
        .ok_or(WaitEnd::Stopped)?;
    if !task.planned_by_parent
        || task.depends_on.is_empty()
        || task.brief_check.refreshed
        || completed_implement_attempts(&task) > 0
    {
        return Ok(());
    }
    let input = ContractInput::capture(&task);
    let changed = landed_files(app, &task).await;
    let targets: Vec<String> = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .flat_map(|c| check_targets(c))
        .collect();
    let touched = overlapping_path(&task.paths, &changed).is_some()
        || changed.iter().any(|f| targets.iter().any(|t| t == f));
    let ruling = if touched {
        judge_refresh(app, &task, &changed, cancel).await?
    } else {
        RefreshRuling {
            decision: None,
            edit: None,
        }
    };
    settle_refresh(app, &task, &input, ruling, cancel)
}

async fn judge_refresh(
    app: &Arc<App>,
    task: &Task,
    changed: &[String],
    cancel: &CancelToken,
) -> Result<RefreshRuling, WaitEnd> {
    let Some((route_id, route, fallback)) = check_route_for(app, &task.variant()) else {
        return Ok(RefreshRuling {
            decision: Some("Refresh: skipped, the brief-check route is empty".into()),
            edit: None,
        });
    };
    let Some(route) = route else {
        return Ok(RefreshRuling {
            decision: Some(format!(
                "Refresh: skipped, no route on an installed agent CLI for {route_id}"
            )),
            edit: None,
        });
    };
    let prompt = refresh_brief(task, changed);
    let reply = match Judge::new(&route, JudgePurpose::DependencyRefresh)
        .run(app, task, &prompt, cancel)
        .await
    {
        Ok(JudgeRun {
            reply,
            cost,
            timeout_note,
        }) => {
            app.update_task(&task.id, |fresh| {
                fresh.charge(cost);
                fresh.record_decisions(timeout_note);
            })
            .map_err(|error| {
                eprintln!("orch: dependency refresh observations {}: {error}", task.id);
                WaitEnd::Stopped
            })?;
            reply
        }
        Err(RunError::Cancelled) => return Err(WaitEnd::Stopped),
        Err(RunError::Io(msg) | RunError::NotFound(msg)) => Err(msg),
    };
    Ok(resolve_refresh(app, task, reply, fallback))
}

fn parsed_reply(reply: Result<String, String>) -> Result<serde_json::Value, String> {
    reply.and_then(|text| {
        let (start, end) = (text.find('{'), text.rfind('}'));
        let slice = start.zip(end).filter(|(s, e)| s < e);
        let (s, e) = slice.ok_or_else(|| "the reply had no JSON".to_string())?;
        serde_json::from_str(&text[s..=e]).map_err(|_| "the reply was not valid JSON".to_string())
    })
}

fn resolve_refresh(
    app: &Arc<App>,
    task: &Task,
    reply: Result<String, String>,
    fallback: Option<String>,
) -> RefreshRuling {
    let note = fallback.map(|n| format!("Refresh: {n}"));
    let value = match parsed_reply(reply) {
        Ok(value) => value,
        Err(why) => {
            let mut lines: Vec<String> = note.into_iter().collect();
            lines.push(format!(
                "Refresh: no usable answer ({}); kept as drafted",
                truncate_chars(&why, 200)
            ));
            return RefreshRuling {
                decision: Some(lines.join("\n")),
                edit: None,
            };
        }
    };
    let differs = |new: Option<Vec<String>>, old: &[String]| {
        new.filter(|n| *n != old && (old.is_empty() || !n.is_empty()))
    };
    let verify = differs(reply_list(&value, "verify"), &task.verify);
    let final_verify = differs(reply_list(&value, "finalVerify"), &task.final_verify);
    let paths = differs(reply_list(&value, "paths"), &task.paths);
    let amendment = Amendment {
        verify: verify.clone(),
        final_verify: final_verify.clone(),
        ..Default::default()
    };
    let settings = app.settings.read().unwrap().clone();
    let refused = (!amendment.is_empty())
        .then(|| shared_check_amend_refusal(&settings, task, &amendment))
        .flatten();
    let (verify, final_verify) = if refused.is_some() {
        (None, None)
    } else {
        (verify, final_verify)
    };
    let mut fields = Vec::new();
    if verify.is_some() {
        fields.push("verify");
    }
    if final_verify.is_some() {
        fields.push("finalVerify");
    }
    if paths.is_some() {
        fields.push("paths");
    }
    let mut decision = match (&refused, fields.is_empty()) {
        (Some(why), _) => format!("Refresh: refused, kept the old commands ({why})"),
        (None, true) => "Refresh: no change".to_string(),
        (None, false) => format!("Refresh: changed {}", fields.join(", ")),
    };
    if refused.is_some() && !fields.is_empty() {
        decision.push_str(&format!("; changed {}", fields.join(", ")));
    }
    let lines: Vec<String> = note.into_iter().chain([decision]).collect();
    let amendment = Amendment {
        verify,
        final_verify,
        ..Default::default()
    };
    RefreshRuling {
        decision: Some(lines.join("\n")),
        edit: Some(RefreshEdit { amendment, paths }),
    }
}

#[cfg(test)]
mod tests;
