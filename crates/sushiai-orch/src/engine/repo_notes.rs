//! `repo.notes.*`: owner-approved standing notes per repo, shown to the
//! planner in the Past work section of its brief. The store is
//! `<data>/repo-notes.json`; `repo.notes.add` and `evolution.approve` are the
//! only writers.

use super::*;

impl App {
    /// The repo root of a path, resolved the way `task.create` does.
    pub(super) async fn resolve_repo(&self, repo: &str) -> Result<String, String> {
        let input = PathBuf::from(repo);
        let root = tokio::task::spawn_blocking(move || git::repo_toplevel(&input))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        Ok(root.to_string_lossy().to_string())
    }

    /// Appends a note for the repo containing `repo`; returns it.
    pub(crate) async fn add_repo_note(
        &self,
        repo: &str,
        text: &str,
        source: String,
    ) -> Result<RepoNote, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a note needs text".to_string());
        }
        let root = self.resolve_repo(repo).await?;
        let _guard = self.notes_lock.lock().await;
        let mut all = self.store.load_repo_notes().map_err(|e| e.to_string())?;
        let note = RepoNote {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.to_string(),
            source,
            created_at: now_ms(),
        };
        all.entry(root).or_default().push(note.clone());
        self.store
            .save_repo_notes(&all)
            .map_err(|e| e.to_string())?;
        Ok(note)
    }

    /// `repo.notes.list {repo}`.
    pub(super) async fn handle_repo_notes_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            repo: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let root = self.resolve_repo(&p.repo).await?;
        let _guard = self.notes_lock.lock().await;
        let notes = self
            .store
            .load_repo_notes()
            .map_err(|e| e.to_string())?
            .remove(&root)
            .unwrap_or_default();
        Ok(json!({"repo": root, "notes": notes}))
    }

    /// `repo.notes.add {repo, text}`: the source is always `owner`.
    pub(super) async fn handle_repo_notes_add(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            repo: String,
            text: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let note = self
            .add_repo_note(&p.repo, &p.text, "owner".to_string())
            .await?;
        serde_json::to_value(&note).map_err(|e| e.to_string())
    }

    /// `repo.notes.remove {repo, id}`: an unknown id is an error.
    pub(super) async fn handle_repo_notes_remove(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            repo: String,
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let root = self.resolve_repo(&p.repo).await?;
        let _guard = self.notes_lock.lock().await;
        let mut all = self.store.load_repo_notes().map_err(|e| e.to_string())?;
        let list = all.entry(root.clone()).or_default();
        let before = list.len();
        list.retain(|n| n.id != p.id);
        if list.len() == before {
            return Err(format!("unknown note: {}", p.id));
        }
        if list.is_empty() {
            all.remove(&root);
        }
        self.store
            .save_repo_notes(&all)
            .map_err(|e| e.to_string())?;
        Ok(json!({"removed": p.id}))
    }
}
