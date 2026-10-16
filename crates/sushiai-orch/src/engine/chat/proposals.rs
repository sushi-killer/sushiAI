use super::api::repo_param;
use super::sessions::ChatStore;
use super::store::{broadcast, load, read_store, release_turn, store_path, BUSY};
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::routing::Tier;
use crate::store;
use crate::task::state::TaskStatus;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;

/// What a proposed task waits for: the 1-based position of another task in the
/// same list, or the id of an existing task.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum ProposalDep {
    Index(usize),
    Id(String),
}

/// One row of a proposal. `task_id` is set once the row became a task,
/// `skipped` once the owner passed on it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProposedTask {
    pub(super) title: String,
    #[serde(default)]
    pub(super) goal: String,
    #[serde(
        default,
        deserialize_with = "crate::engine::planning::serde::lenient_criteria"
    )]
    pub(super) criteria: Vec<String>,
    #[serde(default, deserialize_with = "lenient_deps")]
    pub(super) depends_on: Vec<ProposalDep>,
    #[serde(
        default,
        deserialize_with = "crate::engine::planning::serde::lenient_tier"
    )]
    pub(super) tier: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) task_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) skipped: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(super) struct ChatProposal {
    /// Names the parent task of a proposal of two or more rows.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) feature: String,
    /// That parent, once the first rows were created.
    #[serde(
        default,
        rename = "featureTaskId",
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) feature_task_id: Option<String>,
    pub(super) tasks: Vec<ProposedTask>,
}

impl<'de> Deserialize<'de> for ProposalDep {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => n
                .as_u64()
                .map(|n| ProposalDep::Index(n as usize))
                .ok_or_else(|| D::Error::custom("bad index")),
            // A model may quote the number; task ids are never all digits.
            serde_json::Value::String(s) => {
                let s = s.trim();
                match s.parse::<usize>() {
                    Ok(n) => Ok(ProposalDep::Index(n)),
                    Err(_) if !s.is_empty() => Ok(ProposalDep::Id(s.to_string())),
                    Err(_) => Err(D::Error::custom("empty dependency")),
                }
            }
            _ => Err(D::Error::custom("bad dependency")),
        }
    }
}

fn lenient_deps<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ProposalDep>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = Deserialize::deserialize(d).unwrap_or(None);
    let mut deps: Vec<ProposalDep> = Vec::new();
    for dep in v
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| serde_json::from_value::<ProposalDep>(d).ok())
    {
        if !deps.contains(&dep) {
            deps.push(dep);
        }
    }
    Ok(deps)
}

impl ChatProposal {
    /// The `tasks` list of a `sushi-draft` block. Any row that is not a task
    /// with a title, an index dependency that points outside the list or at
    /// itself, or a dependency cycle drops the whole list.
    pub(super) fn parse(value: &serde_json::Value, feature: Option<&str>) -> Option<ChatProposal> {
        let rows = value.as_array().filter(|rows| !rows.is_empty())?;
        let mut tasks = Vec::with_capacity(rows.len());
        for row in rows {
            let mut task = serde_json::from_value::<ProposedTask>(row.clone()).ok()?;
            task.title = task.title.trim().to_string();
            if task.title.is_empty() {
                return None;
            }
            task.goal = task.goal.trim().to_string();
            task.task_id = None;
            task.skipped = false;
            tasks.push(task);
        }
        let proposal = ChatProposal {
            feature: feature.unwrap_or_default().trim().to_string(),
            feature_task_id: None,
            tasks,
        };
        proposal.acyclic().then_some(proposal)
    }

    /// The 0-based rows a row depends on, `None` for an index out of range or
    /// pointing at the row itself.
    fn row_deps(&self, row: usize) -> Option<Vec<usize>> {
        let mut deps = Vec::new();
        for dep in &self.tasks[row].depends_on {
            if let ProposalDep::Index(n) = dep {
                if *n == 0 || *n > self.tasks.len() || *n - 1 == row {
                    return None;
                }
                deps.push(*n - 1);
            }
        }
        Some(deps)
    }

    fn acyclic(&self) -> bool {
        let all: Vec<usize> = (0..self.tasks.len()).collect();
        self.order(&all).is_some()
    }

    /// The given rows so that every row follows the rows it depends on (among
    /// the given ones); `None` on a bad index or a cycle.
    fn order(&self, rows: &[usize]) -> Option<Vec<usize>> {
        let mut ordered = Vec::new();
        let mut left = rows.to_vec();
        while !left.is_empty() {
            let ready = left.iter().position(|&row| {
                self.row_deps(row)
                    .is_some_and(|deps| deps.iter().all(|d| !left.contains(d)))
            })?;
            ordered.push(left.remove(ready));
        }
        Some(ordered)
    }
}

/// Serialises `chat.createProposal`, so a double click cannot create a row twice.
static PROPOSAL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A list of 1-based row numbers from a request, deduplicated.
fn row_numbers(value: &serde_json::Value, len: usize) -> Result<Vec<usize>, String> {
    let list = value
        .as_array()
        .ok_or("indices must be a list of row numbers")?;
    let mut rows: Vec<usize> = Vec::new();
    for n in list {
        let n = n
            .as_u64()
            .map(|n| n as usize)
            .filter(|n| (1..=len).contains(n))
            .ok_or_else(|| format!("row numbers run from 1 to {len}"))?;
        if !rows.contains(&(n - 1)) {
            rows.push(n - 1);
        }
    }
    Ok(rows)
}

/// The proposal row `messageId` carries, with the session it lives in.
fn find_proposal(chats: &ChatStore, message_id: &str) -> Result<(String, ChatProposal), String> {
    chats
        .sessions
        .iter()
        .find_map(|s| {
            let message = s.messages.iter().find(|m| m.id == message_id)?;
            Some((s.id.clone(), message.proposal.clone()))
        })
        .ok_or_else(|| "no such chat message".to_string())
        .and_then(|(session, proposal)| {
            Ok((
                session,
                proposal.ok_or_else(|| "that message proposes no tasks".to_string())?,
            ))
        })
}

/// Creates `rows` (0-based, dependencies first) as tasks, recording each new
/// task's id on its row as soon as it exists. A proposal of two or more rows
/// is one feature: the first rows created make its parent task, recorded on
/// the proposal, and every row created from it, now or later, is a part that
/// branches from and lands on that parent's branch. A one-row proposal stays a
/// task of its own.
struct ProposalTaskCreation<'a> {
    repo: &'a str,
    backlog: bool,
    task_mcp: Option<&'a serde_json::Value>,
    parent: Option<&'a str>,
}

async fn create_rows(
    app: &App,
    proposal: &mut ChatProposal,
    rows: &[usize],
    creation: &ProposalTaskCreation<'_>,
    created: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    let backlog = creation.backlog;
    let mut fresh = None;
    let parent = if proposal.tasks.len() < 2 {
        None
    } else if let Some(id) = live_feature(app, proposal) {
        Some(id)
    } else {
        let id = create_feature(app, proposal, creation).await?;
        created.push(json!({"feature": true, "taskId": id}));
        proposal.feature_task_id = Some(id.clone());
        fresh = Some(id.clone());
        Some(id)
    };
    let made = create_parts(
        app,
        proposal,
        rows,
        &ProposalTaskCreation {
            parent: parent.as_deref(),
            ..*creation
        },
        created,
    )
    .await;
    if let Some(id) = fresh {
        if made.is_err() && !created.iter().any(|c| c.get("index").is_some()) {
            // No part exists: a childless parent would implement its own
            // placeholder goal.
            let _ = app.handle_task_delete(json!({"id": id})).await;
            proposal.feature_task_id = None;
            created.retain(|c| c["taskId"] != json!(id));
        } else if backlog {
            // Out of `later` even when a part failed, so the parts created
            // are not stranded.
            app.handle_task_backlog(json!({"id": id, "bucket": "next"}))
                .await?;
        } else {
            app.handle_task_start(json!({"id": id})).await?;
        }
    }
    made
}

/// The proposal's parent while new parts can still join it.
fn live_feature(app: &App, proposal: &ChatProposal) -> Option<String> {
    let id = proposal.feature_task_id.as_deref()?;
    let task = app.store.load_task(id).ok().flatten()?;
    (!task.archived && !matches!(task.status, TaskStatus::Done | TaskStatus::Landing))
        .then_some(task.id.clone())
}

/// The parent of a proposal's rows. Backlogged, it waits in `later` until its
/// parts exist, so the autopilot cannot start it half-built.
async fn create_feature(
    app: &App,
    proposal: &ChatProposal,
    creation: &ProposalTaskCreation<'_>,
) -> Result<String, String> {
    let ProposalTaskCreation {
        repo,
        backlog,
        task_mcp,
        ..
    } = *creation;
    let titles: Vec<&str> = proposal.tasks.iter().map(|t| t.title.as_str()).collect();
    let title = if proposal.feature.is_empty() {
        format!("{} and {} more", titles[0], titles.len() - 1)
    } else {
        proposal.feature.clone()
    };
    let mut params = json!({
        "repo": repo,
        "title": title,
        "goal": format!("Every part lands on this task's branch: {}", titles.join("; ")),
        "criteria": titles.iter().map(|t| format!("\"{t}\" landed")).collect::<Vec<_>>(),
        "source": "chat",
        "start": false,
    });
    if backlog {
        params["backlog"] = json!({"bucket": "later"});
    }
    if let Some(mcp) = task_mcp {
        params["mcp"] = mcp.clone();
    }
    let made = app.handle_task_create(params).await?;
    made["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "task.create returned no id".to_string())
}

async fn create_parts(
    app: &App,
    proposal: &mut ChatProposal,
    rows: &[usize],
    creation: &ProposalTaskCreation<'_>,
    created: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    let ProposalTaskCreation {
        repo,
        backlog,
        parent,
        task_mcp,
    } = *creation;
    for &row in rows {
        let task = &proposal.tasks[row];
        let depends_on: Vec<String> = task
            .depends_on
            .iter()
            .filter_map(|dep| match dep {
                ProposalDep::Index(n) => proposal.tasks[*n - 1].task_id.clone(),
                ProposalDep::Id(id) => Some(id.clone()),
            })
            .collect();
        let mut params = json!({
            "repo": repo,
            "title": task.title,
            "goal": if task.goal.is_empty() { &task.title } else { &task.goal },
            "criteria": task.criteria,
            "dependsOn": depends_on,
            "source": "chat",
            "start": !backlog,
        });
        if let Some(parent) = parent {
            params["parent"] = json!(parent);
        } else if backlog {
            params["backlog"] = json!({"bucket": "next"});
        }
        if let Some(mcp) = task_mcp {
            params["mcp"] = mcp.clone();
        }
        let made = app.handle_task_create(params).await?;
        let id = made["id"]
            .as_str()
            .ok_or("task.create returned no id")?
            .to_string();
        created.push(json!({"index": row + 1, "taskId": id}));
        proposal.tasks[row].task_id = Some(id);
        proposal.tasks[row].skipped = false;
    }
    Ok(())
}

/// Turns the chosen rows of a reply's proposal into tasks, dependencies
/// first, and records what happened on the message: each created row keeps its
/// task id, the rows the owner passed on read as skipped. Rows that already
/// became tasks are left alone, so asking twice creates nothing twice.
///
/// Params: `repo`, `messageId`, `indices` (1-based rows to create),
/// `backlog` (park them in the plan backlog instead of starting them) and
/// `skip` (1-based rows to record as skipped; every other unresolved row
/// without it).
pub(in crate::engine) async fn handle_create_proposal(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let message_id = params
        .get("messageId")
        .and_then(|v| v.as_str())
        .ok_or("messageId is required")?
        .to_string();
    let backlog = params
        .get("backlog")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let _one_at_a_time = PROPOSAL_LOCK.lock().await;
    // Holding the turn slot keeps a turn from starting and writing its own
    // copy of the session over the rows recorded here.
    {
        let mut turns = app.chat_turns.lock().unwrap();
        if turns.contains_key(&repo) {
            return Err(BUSY.to_string());
        }
        turns.insert(repo.clone(), CancelToken::new());
    }
    let outcome = create_proposal(app, &repo, &message_id, &params, backlog).await;
    release_turn(app, &repo);
    let (chats, created) = outcome?;
    broadcast(app, &chats);
    let (_, proposal) = find_proposal(&chats, &message_id)?;
    Ok(json!({"proposal": proposal, "created": created}))
}

async fn create_proposal(
    app: &App,
    repo: &str,
    message_id: &str,
    params: &serde_json::Value,
    backlog: bool,
) -> Result<(ChatStore, Vec<serde_json::Value>), String> {
    let chats = load(app, repo);
    let (session_id, mut proposal) = find_proposal(&chats, message_id)?;
    let len = proposal.tasks.len();
    let chosen = row_numbers(params.get("indices").ok_or("indices is required")?, len)?;
    let passed = match params.get("skip") {
        Some(skip) => row_numbers(skip, len)?,
        None => (0..len).filter(|row| !chosen.contains(row)).collect(),
    };
    let todo: Vec<usize> = chosen
        .iter()
        .copied()
        .filter(|&row| proposal.tasks[row].task_id.is_none())
        .collect();
    let ordered = proposal
        .order(&todo)
        .ok_or("the proposed tasks depend on each other in a loop")?;
    for &row in &ordered {
        for dep in proposal.row_deps(row).unwrap_or_default() {
            if proposal.tasks[dep].task_id.is_none() && !todo.contains(&dep) {
                return Err(format!(
                    "\"{}\" depends on \"{}\", which is not created or selected",
                    proposal.tasks[row].title, proposal.tasks[dep].title
                ));
            }
        }
    }
    let task_mcp = chats
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .and_then(|s| s.task_mcp.clone());
    let mut created = Vec::new();
    let made = create_rows(
        app,
        &mut proposal,
        &ordered,
        &ProposalTaskCreation {
            repo,
            backlog,
            task_mcp: task_mcp.as_ref(),
            parent: None,
        },
        &mut created,
    )
    .await;
    if made.is_ok() {
        for (row, task) in proposal.tasks.iter_mut().enumerate() {
            if task.task_id.is_none() {
                task.skipped = passed.contains(&row) && !chosen.contains(&row);
            }
        }
    }
    // Whatever was created is recorded, even when a later row failed.
    let chats = {
        let _turns = app.chat_turns.lock().unwrap();
        let mut chats = read_store(app, repo);
        if let Some(message) = chats
            .sessions
            .iter_mut()
            .find(|s| s.id == session_id)
            .and_then(|s| s.messages.iter_mut().find(|m| m.id == message_id))
        {
            message.proposal = Some(proposal);
        }
        // The turn slot is only a placeholder that serialises writers; no
        // turn runs, so nothing stored or broadcast may say busy.
        chats.mark_busy(repo, &HashMap::new());
        store::files::write_json_atomic(&store_path(app, repo), &chats)
            .map_err(|e| e.to_string())?;
        chats
    };
    match made {
        Ok(()) => Ok((chats, created)),
        Err(error) => {
            broadcast(app, &chats);
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "tests/proposals.rs"]
mod tests;
