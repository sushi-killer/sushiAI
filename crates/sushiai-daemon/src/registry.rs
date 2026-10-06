use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use sushiai_core::StateFile;
use sushiai_protocol::{Notification, SessionInfo, SessionStatus};
use tokio::sync::broadcast;

use crate::home::Home;
use crate::session::Handle;

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
/// ponytail: step-1 shortcut. A `Mutex` that is never held across an await, with the state
/// file written synchronously under it. Upgrade path: a catalog actor that owns this map.
pub struct Registry {
    pub home: Home,
    pub events: broadcast::Sender<Notification>,
    catalog: Mutex<Catalog>,
}

impl Registry {
    pub fn new(home: Home) -> Self {
        Registry {
            home,
            events: broadcast::channel(256).0,
            catalog: Mutex::new(Catalog::default()),
        }
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
        let sessions = catalog
            .sessions
            .values()
            .map(|e| {
                let mut info = e.info.clone();
                // A detached session may still run; the state file says so.
                if info.status == SessionStatus::Detached {
                    info.status = SessionStatus::Running;
                }
                info
            })
            .collect();
        if let Err(e) = self.write_state(&StateFile::new(sessions)) {
            tracing::warn!("cannot write state file: {e}");
        }
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.catalog()
            .sessions
            .values()
            .map(|e| e.info.clone())
            .collect()
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

    /// Temp file, fsync, rename, fsync of the directory: a crash leaves the old or the new
    /// file, never a partial one. An error never writes an empty file.
    fn write_state(&self, state: &StateFile) -> std::io::Result<()> {
        let bytes = state.to_bytes().map_err(std::io::Error::other)?;
        let path = self.home.state();
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
        fs::rename(&tmp, &path)?;
        if let Some(dir) = path.parent() {
            fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
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
        }
    }

    #[test]
    fn keeps_only_the_last_fifty_exited_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::new(Home::new(dir.path().to_path_buf()));
        for id in 0..60 {
            registry.update(exited(id));
        }
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
        let saved = StateFile::from_bytes(&fs::read(dir.path().join("state.json")).expect("read"))
            .expect("state");
        assert_eq!(saved.sessions[0].status, SessionStatus::Running);
        assert_eq!(registry.list()[0].status, SessionStatus::Detached);
    }
}
