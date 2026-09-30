use super::*;

/// Largest single deliverable that is copied out of a worktree.
const MAX_DELIVERABLE_BYTES: u64 = 64 * 1024 * 1024;

/// Relative file paths the task's goal, criteria or verify commands name.
fn named_paths(task: &Task) -> Vec<String> {
    let texts = std::iter::once(&task.goal)
        .chain(task.criteria.iter())
        .chain(task.verify.iter())
        .chain(task.final_verify.iter());
    let mut found: Vec<String> = Vec::new();
    for text in texts {
        for token in text.split(|c: char| c.is_whitespace() || "`\"'(),;<>[]{}=|&*".contains(c)) {
            let token = token.trim_end_matches(['.', ':', '!', '?']);
            let token = token.strip_prefix("./").unwrap_or(token);
            let plain = !token.is_empty()
                && !token.starts_with('/')
                && !token.starts_with(".git/")
                && !token.contains(':')
                && !Path::new(token)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)));
            if plain && !found.iter().any(|f| f == token) {
                found.push(token.to_string());
            }
        }
    }
    found
}

/// Copies every file the task names that exists in its worktree but is not
/// committed (gitignored or untracked) into `dest_root`, keeping relative
/// paths; once the task has landed, also into the same relative path of the
/// repo's main working copy when that path is gitignored there and free.
/// Returns the task's deliverables as they now stand.
pub(super) fn collect(task: &Task, dest_root: &Path) -> Vec<Deliverable> {
    let worktree = Path::new(&task.worktree);
    let repo = Path::new(&task.repo);
    let mut list = task.deliverables.clone();
    if worktree.join(".git").exists() {
        let root = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
        for rel in named_paths(task) {
            let src = worktree.join(&rel);
            let Ok(meta) = std::fs::symlink_metadata(&src) else {
                continue;
            };
            let inside = std::fs::canonicalize(&src).is_ok_and(|p| p.starts_with(&root));
            if !meta.is_file()
                || !inside
                || meta.len() > MAX_DELIVERABLE_BYTES
                || crate::report::is_image(&rel)
                || git::is_committed_clean(worktree, &rel)
            {
                continue;
            }
            let dest = dest_root.join(&rel);
            let saved = dest
                .parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
                && std::fs::copy(&src, &dest).is_ok();
            if !saved {
                continue;
            }
            match list.iter_mut().find(|d| d.path == rel) {
                Some(d) => d.saved = dest.display().to_string(),
                None => list.push(Deliverable {
                    path: rel,
                    saved: dest.display().to_string(),
                    placed: None,
                }),
            }
        }
    }
    if task.landed_sha.is_some() {
        for d in list.iter_mut().filter(|d| d.placed.is_none()) {
            let target = repo.join(&d.path);
            if std::fs::symlink_metadata(&target).is_ok()
                || git::is_tracked(repo, &d.path)
                || !git::is_git_ignored(repo, &d.path)
            {
                continue;
            }
            let copied = target
                .parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
                && std::fs::copy(&d.saved, &target).is_ok();
            if copied {
                d.placed = Some(target.display().to_string());
            }
        }
    }
    list
}

impl App {
    /// Saves the task's uncommitted named files (see [`collect`]) and records
    /// them on `task`; the caller saves it.
    pub(super) async fn collect_deliverables(&self, task: &mut Task) {
        let snapshot = task.clone();
        let dest = self.store.task_dir(&task.id).join("deliverables");
        let list = tokio::task::spawn_blocking(move || collect(&snapshot, &dest))
            .await
            .unwrap_or_else(|_| task.deliverables.clone());
        if list != task.deliverables {
            task.decisions.push(format!(
                "Deliverables: {}",
                list.iter()
                    .map(|d| match &d.placed {
                        Some(p) => format!("{} (kept, and placed at {p})", d.path),
                        None => format!("{} (kept at {})", d.path, d.saved),
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            task.deliverables = list;
        }
    }
}

impl App {
    /// `task.openDeliverable {id, path}`: opens the kept copy of one of the
    /// task's deliverables with the system's default application.
    pub(super) async fn handle_task_open_deliverable(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            path: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let d = task
            .deliverables
            .iter()
            .find(|d| d.path == p.path)
            .ok_or_else(|| "not a deliverable of this task".to_string())?;
        let file = d
            .placed
            .as_ref()
            .filter(|f| Path::new(f).is_file())
            .unwrap_or(&d.saved)
            .clone();
        if !Path::new(&file).is_file() {
            return Err("the deliverable file is gone".to_string());
        }
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(&file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("{opener}: {e}"))?;
        Ok(json!({"opened": file}))
    }
}
