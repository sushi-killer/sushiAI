//! The routine store: `<root>/<id>/` with `routine.json`, `brief.md`, `state.json` and
//! `runs/<n>/{run.json,result.json}`. Every write is atomic (temp file, rename).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::argv::{check_access_tool, check_extra_args};
use crate::model::{valid_id, Access, Agent, HowItRuns, Limits, Routine, When};
use crate::validate::MODES;

/// Why a routine whose files do not match what the app saved is refused.
pub const CHANGED_OUTSIDE: &str = "routine changed outside the app; re-save it";

/// Runs kept per routine; older ones are removed when a run starts.
pub const KEEP_RUNS: usize = 30;

/// The folder of a run's `sushiai ask`: request files `<id>.req` go in, answer files
/// `<id>.ans` come out. A run reaches only this channel, never the daemon socket. Under
/// `work/`, the one folder a sandboxed run may write. The owner's "yes" is never recorded
/// here (see [`Store::asks_dir`]).
pub fn ask_inbox(routine_dir: &Path) -> PathBuf {
    routine_dir.join("work").join("asks")
}

/// A request or answer id is one plain token, so it cannot name a path outside the inbox.
pub fn plain_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    /// Keeps a load from seeing a save half written.
    gate: Arc<Mutex<()>>,
}

/// Makes every temp file name unique inside this process.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!(
        "tmp{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store {
            root,
            gate: Arc::new(Mutex::new(())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn runs_dir(&self, id: &str) -> PathBuf {
        self.dir(id).join("runs")
    }

    pub fn run_dir(&self, id: &str, n: u64) -> PathBuf {
        self.runs_dir(id).join(n.to_string())
    }

    /// Where a scheduled Codex run writes its last message (the daemon starts it from one
    /// stored command line, so the path cannot differ per run).
    pub fn last_message(&self, id: &str) -> PathBuf {
        self.dir(id).join("last-message.json")
    }

    /// The last-message file of a run the module starts itself (dry run, retry): one per run.
    pub fn run_last_message(&self, id: &str, n: u64, retry: bool) -> PathBuf {
        let name = if retry {
            "last-message-retry.json"
        } else {
            "last-message.json"
        };
        self.run_dir(id, n).join(name)
    }

    /// Where `sushiai propose` queues for one session. Under `work/`, the only folder a Codex
    /// run may write; one folder per session keeps runs apart.
    pub fn proposals_dir(&self, id: &str, session: &str) -> PathBuf {
        self.work_dir(id).join("proposals").join(session)
    }

    /// Where an applied proposal is built: a git worktree that is removed afterwards.
    pub fn apply_dir(&self, id: &str, n: u64) -> PathBuf {
        self.dir(id).join(format!("apply-{n}"))
    }

    pub fn schema_file(&self, id: &str) -> PathBuf {
        self.dir(id).join("result.schema.json")
    }

    pub fn work_dir(&self, id: &str) -> PathBuf {
        self.dir(id).join("work")
    }

    pub fn worktree_dir(&self, id: &str) -> PathBuf {
        self.dir(id).join("worktree")
    }

    /// Where a clone of a project's remote lives when a multi-project routine finds no
    /// checkout of it on its host.
    pub fn project_clone_dir(&self, id: &str, project: &str) -> PathBuf {
        let slug: String = project
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir(id).join("projects").join(slug)
    }

    pub fn exists(&self, id: &str) -> bool {
        valid_id(id) && self.dir(id).join("routine.json").is_file()
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| self.exists(n))
            .collect();
        ids.sort();
        ids
    }

    /// The routine as the app saved it. A file that is not a valid routine, or that differs
    /// from the hash recorded at the last save, is unusable: the run that would use it is
    /// refused. `routine.json` is writable by whoever can write the folder; the hash lives in
    /// `state/`, which a run cannot write.
    pub fn load(&self, id: &str) -> Result<Routine, String> {
        if !valid_id(id) {
            return Err(format!("`{id}` is not a routine id"));
        }
        let _g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let text = fs::read_to_string(self.dir(id).join("routine.json"))
            .map_err(|_| format!("no routine {id}"))?;
        let routine: Routine =
            serde_json::from_str(&text).map_err(|e| format!("routine {id} is unreadable: {e}"))?;
        if routine.id != id {
            return Err(format!(
                "routine {id}: id {} does not match its folder",
                routine.id
            ));
        }
        if !MODES.contains(&routine.how_it_runs.mode.as_str()) {
            return Err(format!(
                "routine {id}: unknown mode {}",
                routine.how_it_runs.mode
            ));
        }
        for a in &routine.access {
            check_access_tool(&a.tool).map_err(|m| format!("routine {id}: {m}"))?;
        }
        check_extra_args(&routine.agent.extra_args, &self.dir(id))
            .map_err(|m| format!("routine {id}: {m}"))?;
        let brief = fs::read_to_string(self.dir(id).join("brief.md")).unwrap_or_default();
        let saved = Self::read_json(&self.state_dir(id).join("saved.json"))
            .and_then(|v| v["hash"].as_str().map(String::from));
        // A file saved before `where.projects` carries `where.project`: its recorded hash is
        // over that older form, so it is checked in that form and written in the new one at
        // the next save.
        let legacy = serde_json::from_str::<Value>(&text)
            .is_ok_and(|v| v["where"].get("project").is_some_and(Value::is_string));
        let expected = if legacy {
            Self::legacy_hash(&routine, &brief)
        } else {
            Self::hash(&routine, &brief)
        };
        if saved.as_deref() != Some(expected.as_str()) {
            return Err(format!("routine {id}: {CHANGED_OUTSIDE}"));
        }
        Ok(routine)
    }

    pub fn brief(&self, id: &str) -> String {
        fs::read_to_string(self.dir(id).join("brief.md")).unwrap_or_default()
    }

    /// The daemon-owned folder: a run's sandbox never writes here (it writes `work/` only).
    pub fn state_dir(&self, id: &str) -> PathBuf {
        self.dir(id).join("state")
    }

    /// Writes the routine and brief and records their hash in `state/saved.json`. Only the
    /// app's own paths (save, grant, enable) call this.
    pub fn save(&self, routine: &Routine, brief: &str) -> io::Result<()> {
        let _g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(&routine.id);
        let json = serde_json::to_vec_pretty(routine).map_err(io::Error::other)?;
        write_atomic(&dir.join("routine.json"), &json)?;
        write_atomic(&dir.join("brief.md"), brief.as_bytes())?;
        write_atomic(
            &self.state_dir(&routine.id).join("saved.json"),
            json!({"hash": Self::hash(routine, brief)})
                .to_string()
                .as_bytes(),
        )
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<Routine, String> {
        let mut routine = self.load(id)?;
        routine.enabled = enabled;
        self.save(&routine, &self.brief(id))
            .map_err(|e| e.to_string())?;
        Ok(routine)
    }

    pub fn delete(&self, id: &str) -> io::Result<()> {
        if !valid_id(id) {
            return Ok(());
        }
        match fs::remove_dir_all(self.dir(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The hash the daemon's validated snapshot names: routine plus brief.
    pub fn hash(routine: &Routine, brief: &str) -> String {
        Self::hash_bytes(serde_json::to_vec(routine).unwrap_or_default(), brief)
    }

    /// The hash an app that wrote `where.project` recorded, for a routine with at most one
    /// project.
    fn legacy_hash(routine: &Routine, brief: &str) -> String {
        #[derive(Serialize)]
        struct LegacyWhere<'a> {
            host: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            project: Option<&'a str>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct LegacyRoutine<'a> {
            id: &'a str,
            name: &'a str,
            #[serde(rename = "where")]
            place: LegacyWhere<'a>,
            when: &'a When,
            agent: &'a Agent,
            how_it_runs: &'a HowItRuns,
            limits: &'a Limits,
            #[serde(skip_serializing_if = "<[Access]>::is_empty")]
            access: &'a [Access],
            enabled: bool,
        }
        let old = LegacyRoutine {
            id: &routine.id,
            name: &routine.name,
            place: LegacyWhere {
                host: &routine.place.host,
                project: routine.place.projects.first().map(String::as_str),
            },
            when: &routine.when,
            agent: &routine.agent,
            how_it_runs: &routine.how_it_runs,
            limits: &routine.limits,
            access: &routine.access,
            enabled: routine.enabled,
        };
        Self::hash_bytes(serde_json::to_vec(&old).unwrap_or_default(), brief)
    }

    fn hash_bytes(routine: Vec<u8>, brief: &str) -> String {
        let mut h = Sha256::new();
        h.update(routine);
        h.update([0]);
        h.update(brief.as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Numbers of the kept runs, newest first.
    pub fn run_numbers(&self, id: &str) -> Vec<u64> {
        let mut ns: Vec<u64> = fs::read_dir(self.runs_dir(id))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        ns.sort_unstable_by(|a, b| b.cmp(a));
        ns
    }

    /// Allocates the next run number, creates its folder and removes the runs beyond
    /// [`KEEP_RUNS`]. The folder is created exclusively, so two callers never share a number.
    pub fn new_run(&self, id: &str) -> io::Result<u64> {
        fs::create_dir_all(self.runs_dir(id))?;
        let mut n = self.run_numbers(id).first().map_or(1, |n| n + 1);
        loop {
            match fs::create_dir(self.run_dir(id, n)) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        }
        for old in self.run_numbers(id).into_iter().skip(KEEP_RUNS) {
            let _ = fs::remove_dir_all(self.run_dir(id, old));
        }
        Ok(n)
    }

    fn read_json(path: &Path) -> Option<Value> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    pub fn run(&self, id: &str, n: u64) -> Option<Value> {
        Self::read_json(&self.run_dir(id, n).join("run.json"))
    }

    pub fn write_run(&self, id: &str, n: u64, run: &Value) -> io::Result<()> {
        write_atomic(
            &self.run_dir(id, n).join("run.json"),
            &serde_json::to_vec_pretty(run).map_err(io::Error::other)?,
        )
    }

    pub fn result(&self, id: &str, n: u64) -> Option<Value> {
        Self::read_json(&self.run_dir(id, n).join("result.json"))
    }

    pub fn write_result(&self, id: &str, n: u64, result: &Value) -> io::Result<()> {
        write_atomic(
            &self.run_dir(id, n).join("result.json"),
            &serde_json::to_vec_pretty(result).map_err(io::Error::other)?,
        )
    }

    /// `{consecutiveFailures}`.
    pub fn failures(&self, id: &str) -> u32 {
        Self::read_json(&self.dir(id).join("state.json"))
            .and_then(|v| v.get("consecutiveFailures")?.as_u64())
            .unwrap_or(0) as u32
    }

    fn state(&self, id: &str) -> Value {
        Self::read_json(&self.dir(id).join("state.json"))
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }

    fn write_state(&self, id: &str, state: &Value) -> io::Result<()> {
        write_atomic(
            &self.dir(id).join("state.json"),
            state.to_string().as_bytes(),
        )
    }

    pub fn set_failures(&self, id: &str, n: u32) -> io::Result<()> {
        let mut state = self.state(id);
        state["consecutiveFailures"] = json!(n);
        self.write_state(id, &state)
    }

    /// Accepted asks per action kind (`{"comment": 3}`), kept outside `routine.json` so a
    /// count never changes what the owner reviewed.
    pub fn accepted(&self, id: &str) -> BTreeMap<String, u32> {
        self.state(id)["accepted"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), u32::try_from(v.as_u64()?).ok()?)))
            .collect()
    }

    /// Action kinds that no longer ask: they reached `promoteAfter` accepted asks.
    pub fn promoted(&self, id: &str) -> Vec<String> {
        self.state(id)["promoted"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    }

    /// Adds accepted asks and promotes every kind that reaches `promote_after`. Returns the
    /// kinds promoted by this call.
    pub fn add_accepted(
        &self,
        id: &str,
        added: &BTreeMap<String, u32>,
        promote_after: u32,
    ) -> io::Result<Vec<String>> {
        let mut counts = self.accepted(id);
        let mut promoted = self.promoted(id);
        let mut newly = Vec::new();
        for (kind, n) in added {
            let total = counts.entry(kind.clone()).or_insert(0);
            *total = total.saturating_add(*n);
            if *total >= promote_after && !promoted.contains(kind) {
                promoted.push(kind.clone());
                newly.push(kind.clone());
            }
        }
        let mut state = self.state(id);
        state["accepted"] = json!(counts);
        state["promoted"] = json!(promoted);
        self.write_state(id, &state)?;
        Ok(newly)
    }

    /// Where the owner's answered "yes" of one session is recorded. Under `state/`, outside
    /// every folder a run may write; only the module writes here, after a real owner allow.
    /// A session id that is not a plain token cannot escape the folder.
    pub fn asks_dir(&self, id: &str, session: &str) -> PathBuf {
        let safe = plain_token(session);
        self.state_dir(id)
            .join("asks")
            .join(if safe { session } else { "none" })
    }

    /// Where the files of a proposal that waits for approval are kept.
    pub fn pending_dir(&self, id: &str, n: u64, proposal: usize) -> PathBuf {
        self.run_dir(id, n)
            .join("pending")
            .join(proposal.to_string())
    }

    /// One line in the routine's `log.jsonl`: skips and other notes with a reason.
    pub fn log(&self, id: &str, at_ms: u64, kind: &str, reason: &str) {
        let line = json!({"at": at_ms, "kind": kind, "reason": reason}).to_string();
        let path = self.dir(id).join("log.jsonl");
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_new_runs_never_share_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(dir.path().to_path_buf()));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    (0..10)
                        .map(|_| store.new_run("r1").unwrap())
                        .collect::<Vec<u64>>()
                })
            })
            .collect();
        let mut all: Vec<u64> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        all.sort_unstable();
        let unique = all.len();
        all.dedup();
        assert_eq!(all.len(), unique, "a run number was handed out twice");
        assert_eq!(unique, 80);
    }

    #[test]
    fn concurrent_writes_to_one_file_use_their_own_temp_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(dir.path().to_path_buf()));
        store.new_run("r1").unwrap();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for k in 0..25 {
                        store
                            .write_run("r1", 1, &json!({"n": 1, "writer": i, "k": k}))
                            .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(store.run("r1", 1).unwrap()["n"], 1);
        // No temp file is left behind.
        let leftovers = fs::read_dir(store.run_dir("r1", 1))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }
}
