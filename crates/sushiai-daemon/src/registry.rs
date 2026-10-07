use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use sushiai_core::catalog::project_for_cwd;
use sushiai_core::catalog::{Catalog as Directory, CatalogError};
use sushiai_core::StateFile;
use sushiai_protocol::catalog::SyncResult;
use sushiai_protocol::catalog::{
    CatalogSnapshot, GroupsSync, ProjectsSync, SessionBinding, SessionUpdate,
};
use sushiai_protocol::{method, Notification, SessionInfo, SessionRemoved, SessionStatus};
use tokio::sync::{broadcast, Notify};

use crate::home::Home;
use crate::module::Module;
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
    /// The serialized project and group replica.
    catalog: Option<Vec<u8>>,
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

    fn submit_catalog(&self, bytes: Vec<u8>) {
        self.slot().catalog = Some(bytes);
        self.changed.notify_all();
    }

    /// Waits until everything submitted so far is on disk.
    fn flush(&self) {
        let mut slot = self.slot();
        while slot.next.is_some() || slot.catalog.is_some() || slot.busy {
            slot = self
                .changed
                .wait(slot)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Runs until the registry (the only other owner) is gone.
fn write_loop(writer: Arc<Writer>, path: PathBuf, catalog_path: PathBuf) {
    loop {
        let (state, catalog) = {
            let mut slot = writer.slot();
            while slot.next.is_none() && slot.catalog.is_none() {
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
            (slot.next.take(), slot.catalog.take())
        };
        if let Some(state) = state {
            let written = state
                .to_bytes()
                .map_err(std::io::Error::other)
                .and_then(|bytes| write_atomic(&path, &bytes));
            if let Err(e) = written {
                tracing::warn!("cannot write state file: {e}");
            }
        }
        if let Some(bytes) = catalog {
            if let Err(e) = write_atomic(&catalog_path, &bytes) {
                tracing::warn!("cannot write catalog file: {e}");
            }
        }
        writer.slot().busy = false;
        writer.changed.notify_all();
    }
}

/// Temp file, fsync, rename, fsync of the directory: a crash leaves the old or the new
/// file, never a partial one. An error never writes an empty file.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    // The file holds commands and directories: owner only.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
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

/// Why `remove` failed.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveError {
    NotFound,
    StillRunning,
}

/// The session as clients see it: without the token hash.
fn public(info: &SessionInfo) -> SessionInfo {
    let mut info = info.clone();
    info.agent.token_hash = None;
    info.agent.idempotency_key = None;
    info
}

/// The session catalog: what exists, its last known info, and the state file that mirrors it.
/// Session state itself (screen, seq) lives in the session actors.
///
/// ponytail: step-1 shortcut. A `Mutex` that is never held across an await or an fsync (the
/// state file goes to a writer thread). Upgrade path: a catalog actor that owns this map.
///
/// The registry owns `title`, `project` and `group` of a record: an actor's
/// copy of those fields may be stale, so `update` keeps the stored values.
pub struct Registry {
    pub home: Home,
    /// This host's name in the desktop's catalog (the replica rule of `projects.sync`).
    pub host: String,
    pub events: broadcast::Sender<Notification>,
    catalog: Mutex<Catalog>,
    /// Projects and groups, a replica of the desktop's.
    directory: Mutex<Directory>,
    writer: Arc<Writer>,
    stop: Notify,
    stopping: AtomicBool,
    /// Signalled whenever a restored session settles (reattached, or given up on).
    settled: Notify,
    /// Serializes launches that carry an idempotency key (check, create).
    pub keyed_create: tokio::sync::Mutex<()>,
    /// Hosted modules, set once at startup (none in a bare daemon).
    modules: std::sync::OnceLock<Vec<Arc<dyn Module>>>,
}

impl Registry {
    pub fn new(home: Home) -> Self {
        let host = home.host_name();
        Registry::with_host(home, host)
    }

    pub fn with_host(home: Home, host: String) -> Self {
        let writer = Arc::new(Writer::default());
        let (thread_writer, path, catalog_path) = (writer.clone(), home.state(), home.catalog());
        std::thread::spawn(move || write_loop(thread_writer, path, catalog_path));
        Registry {
            home,
            host,
            events: broadcast::channel(256).0,
            catalog: Mutex::new(Catalog::default()),
            directory: Mutex::new(Directory::new()),
            writer,
            stop: Notify::new(),
            stopping: AtomicBool::new(false),
            settled: Notify::new(),
            keyed_create: tokio::sync::Mutex::new(()),
            modules: std::sync::OnceLock::new(),
        }
    }

    /// Hosts `modules` (once; a second call is ignored).
    pub fn set_modules(&self, modules: Vec<Arc<dyn Module>>) {
        let _ = self.modules.set(modules);
    }

    pub fn modules(&self) -> &[Arc<dyn Module>] {
        self.modules.get().map_or(&[], Vec::as_slice)
    }

    /// The module that owns `namespace`.
    pub fn module(&self, namespace: &str) -> Option<Arc<dyn Module>> {
        self.modules()
            .iter()
            .find(|m| m.namespace() == namespace)
            .cloned()
    }

    /// Asks the daemon to stop. Holders and sessions keep running.
    pub fn request_stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.stop.notify_one();
    }

    /// True once a stop was requested: requests that change anything are refused.
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// True for a session without an actor yet: a restored one whose holder has not answered
    /// (listed `detached`), or a new one whose child already runs while `session.create`
    /// attaches. A session whose actor lost its holder keeps its handle.
    pub fn awaiting_actor(&self, id: &str) -> bool {
        self.catalog()
            .sessions
            .get(id)
            .is_some_and(|e| e.handle.is_none() && e.info.status != SessionStatus::Exited)
    }

    /// Wakes the requests that wait for a session's actor.
    pub fn settle(&self) {
        self.settled.notify_waiters();
    }

    pub fn settled(&self) -> &Notify {
        &self.settled
    }

    pub async fn stopped(&self) {
        self.stop.notified().await;
    }

    fn notify(&self, name: &str, params: impl serde::Serialize) {
        // No receivers is normal when nobody is connected.
        let _ = self.events.send(Notification::new(name, params));
    }

    fn directory(&self) -> MutexGuard<'_, Directory> {
        self.directory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn save_directory(&self, directory: &Directory) {
        match directory.to_bytes() {
            Ok(bytes) => self.writer.submit_catalog(bytes),
            Err(e) => tracing::warn!("cannot serialize the catalog: {e}"),
        }
    }

    /// Replaces the replica with a saved one (daemon start).
    pub fn load_directory(&self, bytes: &[u8]) -> Result<(), CatalogError> {
        *self.directory() = Directory::from_bytes(bytes)?;
        Ok(())
    }

    pub fn sync_projects(&self, sync: &ProjectsSync) -> SyncResult {
        let mut directory = self.directory();
        let result = directory.apply_projects(sync, &self.host);
        if result.applied > 0 {
            self.save_directory(&directory);
        }
        result
    }

    pub fn sync_groups(&self, sync: &GroupsSync) -> SyncResult {
        let mut directory = self.directory();
        let result = directory.apply_groups(sync);
        if result.applied > 0 {
            self.save_directory(&directory);
        }
        result
    }

    /// The live (not deleted) projects and groups, ordered by id.
    pub fn catalog_snapshot(&self) -> CatalogSnapshot {
        let directory = self.directory();
        CatalogSnapshot {
            host: self.host.clone(),
            projects: directory
                .projects
                .values()
                .filter(|p| !p.deleted)
                .cloned()
                .collect(),
            groups: directory
                .groups
                .values()
                .filter(|g| !g.deleted)
                .cloned()
                .collect(),
        }
    }

    /// The binding a new session gets. The project and group are stored as given (opaque: the
    /// desktop owns their meaning, and may not have synced them yet); only an absent project
    /// is filled in, from the folder that holds `cwd`.
    pub fn bind_for_create(
        &self,
        project: Option<String>,
        group: Option<String>,
        cwd: &str,
    ) -> SessionBinding {
        let project = project.or_else(|| project_for_cwd(&self.directory(), &self.host, cwd));
        SessionBinding { project, group }
    }

    /// Applies `session.update`: a missing field is left, `null` clears, anything else is
    /// stored as given. `None` means no such session.
    pub fn update_session(&self, update: SessionUpdate) -> Option<SessionInfo> {
        let mut catalog = self.catalog();
        let entry = catalog.sessions.get_mut(&update.id)?;
        let mut next = entry.info.clone();
        if let Some(project) = update.project {
            next.project = project;
        }
        if let Some(group) = update.group {
            next.group = group;
        }
        if let Some(title) = update.title {
            next.title = Some(title).filter(|t| !t.trim().is_empty());
        }
        if next == entry.info {
            return Some(public(&next));
        }
        entry.info = next;
        let info = public(&entry.info);
        self.save(&catalog);
        self.notify(method::SESSION_UPDATED, &info);
        Some(info)
    }

    /// Sends `session.created` for a session that is up.
    pub fn announce_created(&self, id: &str) {
        let info = self.catalog().sessions.get(id).map(|e| public(&e.info));
        if let Some(info) = info {
            self.notify(method::SESSION_CREATED, &info);
        }
    }

    /// Forgets an exited session.
    pub fn remove(&self, id: &str) -> Result<(), RemoveError> {
        let mut catalog = self.catalog();
        let entry = catalog.sessions.get(id).ok_or(RemoveError::NotFound)?;
        if entry.info.status != SessionStatus::Exited {
            return Err(RemoveError::StillRunning);
        }
        catalog.sessions.remove(id);
        catalog.exited.retain(|e| e != id);
        self.save(&catalog);
        self.notify(method::SESSION_REMOVED, SessionRemoved { id: id.into() });
        Ok(())
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
        self.settle();
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
        let mut info = info;
        match catalog.sessions.get_mut(&id) {
            Some(entry) => {
                let kept = &entry.info;
                info.title = kept.title.clone();
                info.project = kept.project.clone();
                info.group = kept.group.clone();
                entry.info = info;
            }
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
                self.notify(method::SESSION_REMOVED, SessionRemoved { id: old });
            }
        }
        self.save(&catalog);
    }

    /// Sends `session.updated` for a session whose status just settled (after a reattach).
    pub fn announce_updated(&self, id: &str) {
        let info = self.catalog().sessions.get(id).map(|e| public(&e.info));
        if let Some(info) = info {
            self.notify(method::SESSION_UPDATED, &info);
        }
    }

    /// Drops a session found only by its holder socket that nobody answers for.
    pub fn forget_announced(&self, id: &str) {
        self.forget(id);
        self.notify(method::SESSION_REMOVED, SessionRemoved { id: id.into() });
    }

    /// Drops a session that never got going. Rewrites the state file.
    pub fn forget(&self, id: &str) {
        let mut catalog = self.catalog();
        catalog.sessions.remove(id);
        self.save(&catalog);
        drop(catalog);
        self.settle();
    }

    /// The sessions as clients see them: without the token hash.
    pub fn list(&self) -> Vec<SessionInfo> {
        self.catalog()
            .sessions
            .values()
            .map(|e| public(&e.info))
            .collect()
    }

    /// The session launched under `key`, while its record exists.
    pub fn by_key(&self, key: &str) -> Option<String> {
        let catalog = self.catalog();
        let found = catalog
            .sessions
            .values()
            .find(|e| e.info.agent.idempotency_key.as_deref() == Some(key));
        found.map(|e| e.info.id.clone())
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
            project: None,
            group: None,
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
    fn a_new_session_without_its_actor_is_waited_for_and_an_exited_one_is_not() {
        // The child of a new session may call `hook.open` before `session.create` attaches.
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::new(Home::new(dir.path().to_path_buf()));
        let creating = SessionInfo {
            status: SessionStatus::Running,
            exit_code: None,
            ..exited(1)
        };
        registry.update(creating);
        registry.update(exited(2));
        assert!(registry.awaiting_actor("s001"));
        assert!(!registry.awaiting_actor("s002"));
        assert!(!registry.awaiting_actor("nope"));
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

    fn directory(registry: &Registry) {
        let project = |id: &str, host: &str, path: &str| sushiai_protocol::catalog::Project {
            id: id.into(),
            name: id.into(),
            folders: vec![sushiai_protocol::catalog::ProjectFolder {
                host: host.into(),
                path: path.into(),
            }],
            rev: 1,
            updated_at: 1,
            deleted: false,
        };
        let group = |id: &str, project: &str| sushiai_protocol::catalog::Group {
            id: id.into(),
            project_id: project.into(),
            name: id.into(),
            order: 0,
            rev: 1,
            updated_at: 1,
            deleted: false,
        };
        let sync = ProjectsSync {
            host: registry.host.clone(),
            projects: vec![
                project("p1", &registry.host, "/w/a"),
                project("p2", &registry.host, "/w/b"),
            ],
            full: true,
        };
        assert_eq!(registry.sync_projects(&sync).applied, 2);
        let sync = GroupsSync {
            groups: vec![group("g1", "p1"), group("g2", "p2")],
            full: true,
        };
        assert_eq!(registry.sync_groups(&sync).applied, 2);
    }

    fn registry(dir: &tempfile::TempDir) -> Registry {
        Registry::with_host(Home::new(dir.path().to_path_buf()), "devbox".into())
    }

    fn running(id: &str) -> SessionInfo {
        SessionInfo {
            status: SessionStatus::Running,
            exit_code: None,
            ..exited(0)
        }
        .with_id(id)
    }

    trait WithId {
        fn with_id(self, id: &str) -> Self;
    }

    impl WithId for SessionInfo {
        fn with_id(mut self, id: &str) -> Self {
            self.id = id.into();
            self
        }
    }

    fn update(id: &str) -> SessionUpdate {
        SessionUpdate {
            id: id.into(),
            project: None,
            group: None,
            title: None,
        }
    }

    #[test]
    fn the_registry_owns_title_and_binding_against_stale_actor_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(&dir);
        directory(&registry);
        registry.update(running("s1"));
        let set = SessionUpdate {
            project: Some(Some("p1".into())),
            group: Some(Some("g1".into())),
            title: Some("kept".into()),
            ..update("s1")
        };
        registry.update_session(set).expect("update");
        // An actor still holds the old copy and saves it.
        registry.update(running("s1"));
        let info = &registry.list()[0];
        assert_eq!(info.project.as_deref(), Some("p1"));
        assert_eq!(info.group.as_deref(), Some("g1"));
        assert_eq!(info.title.as_deref(), Some("kept"));
    }

    #[test]
    fn a_binding_is_stored_as_given_even_when_the_catalog_does_not_know_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A fresh daemon: nothing synced yet.
        let registry = registry(&dir);
        registry.update(running("s1"));
        let set = SessionUpdate {
            project: Some(Some("unsynced".into())),
            group: Some(Some("also-unsynced".into())),
            ..update("s1")
        };
        let info = registry.update_session(set).expect("update");
        assert_eq!(info.project.as_deref(), Some("unsynced"));
        assert_eq!(info.group.as_deref(), Some("also-unsynced"));
        let clear = SessionUpdate {
            project: Some(None),
            ..update("s1")
        };
        let info = registry.update_session(clear).expect("update");
        assert_eq!(info.project, None);
        assert_eq!(info.group.as_deref(), Some("also-unsynced"));
        assert!(registry.update_session(update("missing")).is_none());
    }

    #[test]
    fn create_binds_by_cwd_only_when_no_project_is_given() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(&dir);
        directory(&registry);
        let by_cwd = registry.bind_for_create(None, None, "/w/a/src");
        assert_eq!(by_cwd.project.as_deref(), Some("p1"));
        let given = registry.bind_for_create(Some("p2".into()), None, "/w/a/src");
        assert_eq!(given.project.as_deref(), Some("p2"));
        let none = registry.bind_for_create(None, None, "/w/abc");
        assert_eq!(none.project, None);
        // A group the catalog has never seen is kept as given.
        let opaque = registry.bind_for_create(None, Some("gx".into()), "/w/a");
        assert_eq!(opaque.group.as_deref(), Some("gx"));
    }

    #[test]
    fn the_catalog_is_saved_next_to_the_state_file_and_loads_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = registry(&dir);
        directory(&first);
        first.flush();
        let bytes = fs::read(dir.path().join("catalog.json")).expect("catalog file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("\"catalogVersion\": 1"), "{text}");
        let second = registry(&dir);
        second.load_directory(&bytes).expect("load");
        let bound = second.bind_for_create(None, None, "/w/b");
        assert_eq!(bound.project.as_deref(), Some("p2"));
        // A file without the catalog version is refused.
        assert!(second.load_directory(b"{}").is_err());
    }

    #[test]
    fn only_an_exited_session_is_removed_and_listeners_hear_about_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(&dir);
        let mut events = registry.events.subscribe();
        registry.update(running("live"));
        registry.update(exited(1));
        assert_eq!(registry.remove("live"), Err(RemoveError::StillRunning));
        assert_eq!(registry.remove("nope"), Err(RemoveError::NotFound));
        registry.remove("s001").expect("remove");
        let note = events.try_recv().expect("notification");
        assert_eq!(note.method, method::SESSION_REMOVED);
        assert_eq!(note.params["id"], "s001");
        assert!(!registry.known("s001") && registry.known("live"));
    }

    #[test]
    fn forgetting_the_oldest_exited_sessions_announces_removal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(&dir);
        let mut events = registry.events.subscribe();
        for id in 0..=MAX_EXITED {
            registry.update(exited(id));
        }
        let note = events.try_recv().expect("notification");
        assert_eq!(
            (note.method.as_str(), note.params["id"].as_str()),
            (method::SESSION_REMOVED, Some("s000"))
        );
    }

    #[test]
    fn a_key_finds_its_session_while_the_record_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::new(Home::new(dir.path().to_path_buf()));
        let mut info = exited(1);
        info.status = SessionStatus::Running;
        info.agent.idempotency_key = Some("a".into());
        registry.update(info);
        assert_eq!(registry.by_key("a"), Some("s001".into()));
        assert_eq!(registry.by_key("b"), None);
        assert!(registry
            .list()
            .iter()
            .all(|i| i.agent.idempotency_key.is_none()));
        registry.forget("s001");
        assert_eq!(registry.by_key("a"), None);
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
