use super::files::write_json_atomic;
use super::Store;
use crate::task::Task;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize)]
struct HelperRun {
    stage: String,
    attempt: Option<u32>,
}

pub(super) fn parse_task(text: &str) -> Result<Task, serde_json::Error> {
    let mut task: Task = serde_json::from_str(text)?;
    crate::task::migrations::apply(&mut task);
    crate::verify::repository::migrate_check_sources(&mut task);
    Ok(task)
}

impl Store {
    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("tasks").join(id)
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.task_dir(id).join("task.json")
    }

    pub fn run_dir(&self, id: &str, n: u32) -> PathBuf {
        self.task_dir(id).join("runs").join(n.to_string())
    }

    pub fn helper_run_dir(&self, task_id: &str, stage: &str, seq: u32) -> PathBuf {
        self.task_dir(task_id)
            .join("helpers")
            .join(format!("{stage}-{seq}"))
    }

    pub fn claim_helper_run_dir(
        &self,
        task_id: &str,
        stage: &str,
        attempt: Option<u32>,
    ) -> io::Result<PathBuf> {
        fs::create_dir_all(self.task_dir(task_id).join("helpers"))?;
        for seq in 1..=u32::MAX {
            let dir = self.helper_run_dir(task_id, stage, seq);
            match fs::create_dir(&dir) {
                Ok(()) => {
                    write_json_atomic(
                        &dir.join("helper.json"),
                        &HelperRun {
                            stage: stage.to_string(),
                            attempt,
                        },
                    )?;
                    return Ok(dir);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("helper sequence exhausted"))
    }

    pub fn helper_run_dirs(
        &self,
        task_id: &str,
        stage: &str,
        attempt: Option<u32>,
    ) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.task_dir(task_id).join("helpers")) else {
            return Vec::new();
        };
        let prefix = format!("{stage}-");
        let mut dirs: Vec<(u32, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let seq = name.to_str()?.strip_prefix(&prefix)?.parse::<u32>().ok()?;
                let dir = entry.path();
                let text = fs::read_to_string(dir.join("helper.json")).ok()?;
                let helper: HelperRun = serde_json::from_str(&text).ok()?;
                (helper.stage == stage && helper.attempt == attempt).then_some((seq, dir))
            })
            .collect();
        dirs.sort_by_key(|(seq, _)| *seq);
        dirs.into_iter().map(|(_, dir)| dir).collect()
    }

    pub fn save_task(&self, task: &Task) -> io::Result<()> {
        let dir = self.task_dir(&task.id);
        fs::create_dir_all(&dir)?;
        write_json_atomic(&self.task_path(&task.id), task)?;
        // Every answered question lands in the decision memory (`precedents.rs`).
        crate::precedents::sync(&self.data_dir, task);
        Ok(())
    }

    pub fn load_task(&self, id: &str) -> io::Result<Option<Task>> {
        let path = self.task_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let task = parse_task(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(task))
    }

    pub fn list_tasks(&self) -> io::Result<Vec<Task>> {
        let tasks_dir = self.data_dir.join("tasks");
        let mut out = Vec::new();
        if !tasks_dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&tasks_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let task_json = entry.path().join("task.json");
            if !task_json.exists() {
                continue;
            }
            let text = match fs::read_to_string(&task_json) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!("orch: reading task {} failed: {e}", task_json.display());
                    continue;
                }
            };
            match parse_task(&text) {
                Ok(task) => out.push(task),
                Err(e) => {
                    eprintln!("orch: parsing task {} failed: {e}", task_json.display());
                }
            }
        }
        out.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        Ok(out)
    }
}
