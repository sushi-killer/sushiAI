use crate::api::validation::validate_task_id;
use crate::engine::lifecycle::carry::{carry_onto_newest_base, carry_relay_onto_parent, BaseCarry};
use crate::engine::planning::graph::overlapping_path;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

impl App {
    fn graph_root(&self, task_id: &str) -> Result<Task, String> {
        let mut id = task_id.to_string();
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(id.clone()) {
                return Err("task parent cycle".into());
            }
            validate_task_id(&self.store, &id)?;
            let task = self
                .store
                .load_task(&id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "task not found".to_string())?;
            match task.parent.clone() {
                Some(parent) => id = parent,
                None => return Ok(task),
            }
        }
    }

    pub(super) async fn sync_graph_base(
        &self,
        task_id: &str,
        force: bool,
    ) -> Result<serde_json::Value, String> {
        let root = self.graph_root(task_id)?;
        let lock = self.landing_lock(&root.id);
        let _landing = lock.lock().await;
        let root = self.graph_root(&root.id)?;
        let base = root
            .base_ref
            .clone()
            .ok_or_else(|| "task has no base branch".to_string())?;
        if root.archived {
            return Err("task is archived".into());
        }
        let (wt, reference) = (PathBuf::from(&root.worktree), base.clone());
        let base_head = tokio::task::spawn_blocking(move || {
            crate::git::history::resolve_commit(&wt, &reference)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        if !force {
            let all = self.repo_tasks(&root.repo.clone());
            let mut ids = vec![root.id.clone()];
            loop {
                let before = ids.len();
                for task in &all {
                    if task.parent.as_ref().is_some_and(|p| ids.contains(p))
                        && !ids.contains(&task.id)
                    {
                        ids.push(task.id.clone());
                    }
                }
                if before == ids.len() {
                    break;
                }
            }
            let mut held: Vec<String> = all
                .iter()
                .filter(|t| ids.contains(&t.id))
                .flat_map(|t| t.paths.clone())
                .collect();
            held.extend(self.leased_paths(&ids));
            let (wt, reference) = (PathBuf::from(&root.worktree), base_head.clone());
            let incoming = tokio::task::spawn_blocking(move || {
                crate::delivery::git::carry::incoming_paths(&wt, &reference)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
            if overlapping_path(&incoming, &held).is_some() {
                return Ok(
                    json!({"id": root.id, "synced": false, "overlap": true, "conflicts": []}),
                );
            }
        }
        let (wt, reference) = (PathBuf::from(&root.worktree), base_head);
        let merged = tokio::task::spawn_blocking(move || {
            crate::delivery::git::carry::merge_base(&wt, &reference)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string());
        match merged {
            Ok(crate::delivery::git::carry::BaseMerge::Unchanged) => {
                Ok(json!({"id": root.id, "synced": false, "conflicts": []}))
            }
            Ok(crate::delivery::git::carry::BaseMerge::Merged { head }) => {
                self.append_decision(
                    &root.id,
                    format!(
                        "Sync: merged {base} into {} at {}",
                        root.branch,
                        short_sha(&head)
                    ),
                );
                Ok(json!({"id": root.id, "synced": true, "head": head, "conflicts": []}))
            }
            Ok(crate::delivery::git::carry::BaseMerge::Conflicts { files }) => {
                self.append_decision(
                    &root.id,
                    format!(
                        "Sync: merging {base} into {} conflicted in {}; merge aborted",
                        root.branch,
                        files.join(", ")
                    ),
                );
                Ok(json!({"id": root.id, "synced": false, "conflicts": files}))
            }
            Err(error) => {
                self.append_decision(&root.id, format!("Sync: could not merge {base}: {error}"));
                Err(error)
            }
        }
    }

    pub(super) async fn carry_part_onto_parent(
        &self,
        task: &mut Task,
        cancel: &CancelToken,
    ) -> BaseCarry {
        let worktree = PathBuf::from(&task.worktree);
        if let Some(chain_base) = task.queue.relay_base.clone() {
            if let Some(parent) = task
                .parent
                .as_ref()
                .and_then(|id| self.store.load_task(id).ok().flatten())
            {
                let base = self
                    .graph_root(&task.id)
                    .ok()
                    .and_then(|t| t.base_ref.clone());
                let (wt, branch, from) =
                    (worktree.clone(), parent.branch.clone(), chain_base.clone());
                let needs_carry = tokio::task::spawn_blocking(move || {
                    let Some(base) =
                        base.and_then(|r| crate::git::history::resolve_commit(&wt, &r).ok())
                    else {
                        return false;
                    };
                    crate::git::history::is_ancestor(&wt, &base, &from) == Some(false)
                        && crate::git::history::is_ancestor(&wt, &base, &branch) == Some(true)
                })
                .await
                .unwrap_or(false);
                if needs_carry {
                    return carry_relay_onto_parent(self, task, &parent, &chain_base, cancel).await;
                }
            }
            return BaseCarry::Ready;
        }
        carry_onto_newest_base(self, task, &worktree, cancel).await
    }

    pub(super) async fn handle_task_sync(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        self.sync_graph_base(&p.id, true).await
    }
}
