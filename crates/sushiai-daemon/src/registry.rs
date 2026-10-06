use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sushiai_core::StateFile;
use sushiai_protocol::{Notification, SessionInfo, SessionStatus};
use tokio::sync::broadcast;

use crate::home::Home;
use crate::session::Handle;

pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Compares the SHA-256 of `token` with a stored hex digest, folding every byte so the time
/// does not depend on where they differ.
fn digest_matches(stored_hex: &str, token: &str) -> bool {
    let Some(stored) = unhex(stored_hex) else {
        return false;
    };
    let given = Sha256::digest(token.as_bytes());
    let diff = stored
        .iter()
        .zip(given.iter())
        .fold(0u8, |a, (x, y)| a | (x ^ y));
    stored.len() == given.len() && diff == 0
}

/// How a record is saved: asks die with the daemon, and a detached session may still run.
fn persisted(info: &SessionInfo) -> SessionInfo {
    let mut info = info.clone();
    info.agent.asks.clear();
    if info.status == SessionStatus::Detached {
        info.status = SessionStatus::Running;
    }
    info
}

/// The state file is written by one thread, never by a session actor: the latest snapshot
/// wins, and each write keeps the temp + fsync + rename + directory fsync steps.
#[derive(Default)]
struct Writer {
    slot: Mutex<Slot>,
    changed: Condvar,
}

#[derive(Default)]
struct Slot {
    next: Option<StateFile>,
    busy: bool,
}

impl Writer {
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn submit(&self, state: StateFile) {
        self.slot().next = Some(state);
        self.changed.notify_all();
    }

    /// Waits until everything submitted so far is on disk.
    fn flush(&self) {
        let mut slot = self.slot();
        while slot.next.is_some() || slot.busy {
            slot = self
                .changed
                .wait(slot)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Runs until the registry (the only other owner) is gone.
fn write_loop(writer: Arc<Writer>, path: PathBuf) {
    loop {
        let state = {
            let mut slot = writer.slot();
            while slot.next.is_none() {
                if Arc::strong_count(&writer) == 1 {
                    return;
                }
                slot = writer
                    .changed
                    .wait_timeout(slot, Duration::from_millis(500))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            slot.busy = true;
            slot.next.take()
        };
        if let Some(state) = state {
            if let Err(e) = write_state(&path, &state) {
                tracing::warn!("cannot write state file: {e}");
            }
        }
        writer.slot().busy = false;
        writer.changed.notify_all();
    }
}

/// Temp file, fsync, rename, fsync of the directory: a crash leaves the old or the new
/// file, never a partial one. An error never writes an empty file.
fn write_state(path: &std::path::Path, state: &StateFile) -> std::io::Result<()> {
    let bytes = state.to_bytes().map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    // The file holds commands and directories: owner only.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// The state file keeps at most this many exited sessions; the oldest are forgotten.
const MAX_EXITED: usize = 50;

struct Entry {
    info: SessionInfo,
    handle: Option<Handle>,
}

#[derive(Default)]
struct Catalog {
    sessions: BTreeMap<String, Entry>,
    /// Ids of exited sessions, oldest first.
    exited: VecDeque<String>,
}

/// The session catalog: what exists, its last known info, and the state file that mirrors it.
/// Session state itself (screen, seq) lives in the session actors.
///
/// ponytail: step-1 shortcut. A `Mutex` that is never held across an await or an fsync (the
/// state file goes to a writer thread). Upgrade path: a catalog actor that owns this map.
pub struct Registry {
    pub home: Home,
    pub events: broadcast::Sender<Notification>,
    catalog: Mutex<Catalog>,
    writer: Arc<Writer>,
}

impl Registry {
    pub fn new(home: Home) -> Self {
        let writer = Arc::new(Writer::default());
        let (thread_writer, path) = (writer.clone(), home.state());
        std::thread::spawn(move || write_loop(thread_writer, path));
        Registry {
            home,
            events: broadcast::channel(256).0,
            catalog: Mutex::new(Catalog::default()),
            writer,
        }
    }

    /// Blocks until the state file shows every change made so far.
    pub fn flush(&self) {
        self.writer.flush();
    }

    fn save(&self, catalog: &Catalog) {
        let sessions = catalog
            .sessions
            .values()
            .map(|e| persisted(&e.info))
            .collect();
        self.writer.submit(StateFile::new(sessions));
    }

    fn catalog(&self) -> MutexGuard<'_, Catalog> {
        self.catalog.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_handle(&self, id: &str, handle: Handle) {
        if let Some(entry) = self.catalog().sessions.get_mut(id) {
            entry.handle = Some(handle);
        }
    }

    pub fn clear_handle(&self, id: &str) {
        if let Some(entry) = self.catalog().sessions.get_mut(id) {
            entry.handle = None;
        }
    }

    /// Inserts or replaces a session's info and rewrites the state file.
    pub fn update(&self, info: SessionInfo) {
        let mut catalog = self.catalog();
        let id = info.id.clone();
        let exited = info.status == SessionStatus::Exited;
        match catalog.sessions.get_mut(&id) {
            Some(entry) => entry.info = info,
            None => {
                catalog
                    .sessions
                    .insert(id.clone(), Entry { info, handle: None });
            }
        }
        if exited && !catalog.exited.contains(&id) {
            catalog.exited.push_back(id);
        }
        while catalog.exited.len() > MAX_EXITED {
            if let Some(old) = catalog.exited.pop_front() {
                catalog.sessions.remove(&old);
            }
        }
        self.save(&catalog);
    }

    /// Drops a session that never got going. Rewrites the state file.
    pub fn forget(&self, id: &str) {
        let mut catalog = self.catalog();
        catalog.sessions.remove(id);
        self.save(&catalog);
    }

    /// The sessions as clients see them: without the token hash.
    pub fn list(&self) -> Vec<SessionInfo> {
        self.catalog()
            .sessions
            .values()
            .map(|e| {
                let mut info = e.info.clone();
                info.agent.token_hash = None;
                info
            })
            .collect()
    }

    /// True when `token` is the one the session was created with. Only a hash is kept.
    pub fn token_matches(&self, id: &str, token: &str) -> bool {
        self.catalog()
            .sessions
            .get(id)
            .and_then(|e| e.info.agent.token_hash.clone())
            .is_some_and(|hash| digest_matches(&hash, token))
    }

    pub fn agent_of(&self, id: &str) -> Option<String> {
        self.catalog()
            .sessions
            .get(id)
            .and_then(|e| e.info.agent.name.clone())
    }

    /// The session that holds the open ask `ask_id`.
    pub fn session_of_ask(&self, ask_id: &str) -> Option<String> {
        let catalog = self.catalog();
        let found = catalog
            .sessions
            .values()
            .find(|e| e.info.agent.asks.iter().any(|a| a.ask_id == ask_id));
        found.map(|e| e.info.id.clone())
    }

    pub fn known(&self, id: &str) -> bool {
        self.catalog().sessions.contains_key(id)
    }

    pub fn handle(&self, id: &str) -> Option<Handle> {
        self.catalog()
            .sessions
            .get(id)
            .and_then(|e| e.handle.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited(id: usize) -> SessionInfo {
        SessionInfo {
            id: format!("s{id:03}"),
            cmd: vec![],
            cwd: String::new(),
            title: None,
            status: SessionStatus::Exited,
            exit_code: Some(0),
            holder_pid: None,
            cols: 80,
            rows: 24,
            agent: Default::default(),
        }
    }

    #[test]
    fn keeps_only_the_last_fifty_exited_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::new(Home::new(dir.path().to_path_buf()));
        for id in 0..60 {
            registry.update(exited(id));
        }
        registry.flush();
        let ids: Vec<String> = registry.list().into_iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), MAX_EXITED);
        assert!(!ids.contains(&"s009".to_string()) && ids.contains(&"s010".to_string()));
        let saved = StateFile::from_bytes(&fs::read(dir.path().join("state.json")).expect("read"))
            .expect("state");
        assert_eq!(saved.sessions.len(), MAX_EXITED);
    }

    #[test]
    fn a_detached_session_is_saved_as_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::new(Home::new(dir.path().to_path_buf()));
        let mut info = exited(1);
        info.status = SessionStatus::Detached;
        registry.update(info);
        registry.flush();
        let saved = StateFile::from_bytes(&fs::read(dir.path().join("state.json")).expect("read"))
            .expect("state");
        assert_eq!(saved.sessions[0].status, SessionStatus::Running);
        assert_eq!(registry.list()[0].status, SessionStatus::Detached);
    }

    #[test]
    fn forgetting_a_session_saves_the_others_like_update_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::new(Home::new(dir.path().to_path_buf()));
        let mut kept = exited(1);
        kept.status = SessionStatus::Detached;
        kept.agent.asks.push(sushiai_protocol::Ask {
            ask_id: "a".into(),
            session: "s001".into(),
            tool: None,
            input: serde_json::Value::Null,
        });
        registry.update(kept);
        registry.update(exited(2));
        registry.forget("s002");
        registry.flush();
        let saved = StateFile::from_bytes(&fs::read(dir.path().join("state.json")).expect("read"))
            .expect("state");
        assert_eq!(saved.sessions.len(), 1);
        assert_eq!(saved.sessions[0].status, SessionStatus::Running);
        assert!(saved.sessions[0].agent.asks.is_empty());
    }

    #[test]
    fn a_token_matches_only_its_own_digest() {
        let good = hash_token("secret");
        assert!(digest_matches(&good, "secret"));
        assert!(!digest_matches(&good, "secreT"));
        assert!(!digest_matches("zz", "secret"));
        assert!(!digest_matches(&good[..good.len() - 2], "secret"));
    }
}
