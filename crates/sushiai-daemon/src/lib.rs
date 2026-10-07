//! The sushiai daemon: serves clients on a unix socket, runs one actor per session and
//! reconnects to surviving holders after a restart.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod agent;
mod binlink;
mod error;
mod foreground;
mod framed;
mod holder;
mod home;
mod launch_store;
mod module;
mod procs;
mod registry;
mod server;
mod session;
mod wake;

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sushiai_core::hibernate::can_hibernate;
use sushiai_core::{mark_exited, mark_hibernated, Screen, StateError, StateFile};
use sushiai_protocol::{
    method, Frame, Hello, Message, Request, SessionInfo, SessionStatus, PROTOCOL_VERSION,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};

pub use agent::ASK_WAIT_SECS;
pub use binlink::{ensure_bin_link, Link};
pub use error::{Error, Result};
pub use home::Home;
pub use module::{BoxFuture, Module, ModuleNotify, ModuleSlot, Reply};
use registry::Registry;

/// Runs the daemon until SIGTERM, SIGINT or `daemon.shutdown`. Holders keep running.
///
/// The home lock is released last: after the runtime has stopped every task, and after a
/// final flush of the state file, so the next daemon never starts on a stale file.
pub fn run_blocking(home: Home, modules: Vec<ModuleSlot>) -> Result<()> {
    let runtime = runtime()?;
    let (lock, registry) = runtime.block_on(run(home, modules))?;
    drop(runtime);
    registry.flush();
    drop(lock);
    Ok(())
}

/// Prints one line about the running daemon; errors when none answers.
pub fn status_blocking(home: Home) -> Result<String> {
    runtime()?.block_on(status(home))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

async fn run(home: Home, modules: Vec<ModuleSlot>) -> Result<(fs::File, Arc<Registry>)> {
    // Hashing the binary takes long in a large build: it runs beside startup, so the first
    // `hello` finds it measured instead of making a client wait.
    std::thread::spawn(|| {
        server::own_build();
    });
    home.ensure()?;
    // The lock lives until the daemon stops: one daemon per home, decided before any recovery.
    let lock = lock_home(&home)?;
    match binlink::ensure_bin_link(home.dir()) {
        Ok(Link::Kept) => tracing::warn!("bin/sushiai exists and is not a symlink; left alone"),
        Ok(_) => {}
        Err(e) => tracing::warn!("cannot link bin/sushiai: {e}"),
    }
    let socket = home.socket();
    let registry = Arc::new(Registry::new(home.clone()));
    registry.load_settings();
    load_catalog(&registry);
    // A newer state schema stops the daemon here, before anything listens.
    let pending = load_state(&registry)?;
    let _ = fs::remove_file(&socket);
    let listener = bind_private(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    tracing::info!("daemon listening on {}", socket.display());
    // The socket answers while the holders come back. Sessions are listed `detached` until their
    // holder answered (running) or was given up on (exited); `session.updated` follows.
    reattach_all(&registry, pending);
    start_modules(&registry, modules)?;

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        () = server::serve(listener, registry.clone()) => {}
        () = prune_loop(registry.clone()) => {}
        () = registry.stopped() => {
            // The response to `daemon.shutdown` is queued; give its connection time to send it.
            tokio::time::sleep(STOP_GRACE).await;
        }
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    let _ = fs::remove_file(&socket);
    // A reboot that follows has something to show for each agent session.
    save_tails(&registry).await;
    stop_modules(&registry).await;
    registry.flush();
    Ok((lock, registry))
}

/// Once an hour: hibernated sessions older than 30 days are forgotten, with their files.
async fn prune_loop(registry: Arc<Registry>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
    tick.tick().await;
    loop {
        tick.tick().await;
        registry.prune_hibernated(agent::now_ms());
    }
}

/// Saves the screen of every running agent session, all at once.
async fn save_tails(registry: &Registry) {
    let saves: Vec<_> = registry
        .handles()
        .into_iter()
        .map(|handle| tokio::spawn(async move { handle.save_tail().await }))
        .collect();
    for save in saves {
        let _ = save.await;
    }
}

/// Builds the hosted modules once the socket answers. Two modules with one namespace are a
/// programming error and stop the daemon before it serves.
fn start_modules(registry: &Arc<Registry>, slots: Vec<ModuleSlot>) -> Result<()> {
    let mut seen = Vec::new();
    let mut modules = Vec::new();
    for slot in slots {
        if seen.contains(&slot.namespace) {
            return Err(Error::Holder(format!(
                "two modules use the namespace {}",
                slot.namespace
            )));
        }
        seen.push(slot.namespace);
        let notify = ModuleNotify::new(slot.namespace, registry.events.clone());
        let module = (slot.build)(notify);
        if module.namespace() != slot.namespace {
            return Err(Error::Holder(format!(
                "module built for {} answers to {}",
                slot.namespace,
                module.namespace()
            )));
        }
        modules.push(module);
    }
    registry.set_modules(modules);
    Ok(())
}

/// How long modules get to stop.
const MODULE_STOP: std::time::Duration = std::time::Duration::from_secs(12);

async fn stop_modules(registry: &Registry) {
    for module in registry.modules() {
        if tokio::time::timeout(MODULE_STOP, module.shutdown())
            .await
            .is_err()
        {
            tracing::warn!("module {} did not stop in time", module.namespace());
        }
    }
}

/// How long a stopping daemon lets queued responses go out.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// Binds the socket with umask 077, so it is never connectable by others, not even briefly.
/// The umask is process-wide: it is restored right after the bind.
fn bind_private(path: &Path) -> Result<UnixListener> {
    // SAFETY: umask(2) has no preconditions.
    let old = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(path);
    // SAFETY: as above.
    unsafe { libc::umask(old) };
    Ok(bound?)
}

/// How long a starting daemon waits for the home lock: the daemon that is stopping releases
/// it a moment after its socket is gone, and a client that starts a daemon right away must not
/// lose that race.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Takes an exclusive `flock` on `<home>/daemon.lock` (retried for up to `LOCK_WAIT`) and
/// writes the pid into it. Another running daemon makes this fail with `AlreadyRunning`.
fn lock_home(home: &Home) -> Result<fs::File> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(home.lock())?;
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    // SAFETY: flock(2) on a descriptor this function owns.
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if std::time::Instant::now() >= deadline {
            return Err(Error::AlreadyRunning(home.socket().display().to_string()));
        }
        std::thread::sleep(LOCK_POLL);
    }
    // Only the lock holder writes, so a refused second daemon never clobbers the pid.
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(file)
}

/// Loads `catalog.json`. The desktop is the master of that replica, so a file that cannot be
/// read (corrupt, or from a newer daemon) is set aside and the desktop syncs again.
fn load_catalog(registry: &Registry) {
    let path = registry.home.catalog();
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    if let Err(e) = registry.load_directory(&bytes) {
        let aside = path.with_extension(format!("json.unreadable-{}", now_secs()));
        tracing::warn!(
            "catalog file is unreadable ({e}); moved to {}",
            aside.display()
        );
        let _ = fs::rename(&path, aside);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A session found in the state file whose holder is not connected yet.
struct Pending {
    info: SessionInfo,
    /// Found only by its holder socket (the state file was corrupt): dropped, not marked
    /// exited, when nobody answers.
    probed: bool,
}

/// Loads the state file. Finished sessions go straight into the registry; running ones go in
/// as `detached` entries and come back as the returned list. A newer schema stops the daemon. A
/// corrupt file is set aside and the catalog is rebuilt from the holder sockets.
fn load_state(registry: &Arc<Registry>) -> Result<Vec<Pending>> {
    let path = registry.home.state();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let (infos, probed) = match StateFile::from_bytes(&bytes) {
        Ok(state) => (state.sessions, false),
        Err(e @ StateError::UnsupportedSchema(_)) => return Err(e.into()),
        Err(StateError::Invalid(e)) => {
            let aside = path.with_extension(format!("json.corrupt-{}", now_secs()));
            tracing::warn!("state file is corrupt ({e}); moved to {}", aside.display());
            fs::rename(&path, aside)?;
            (probe_holders(registry), true)
        }
    };
    let mut pending = Vec::new();
    for info in infos {
        if info.status == SessionStatus::Running {
            let mut listed = info.clone();
            listed.status = SessionStatus::Detached;
            registry.update(listed);
            pending.push(Pending { info, probed });
        } else if info.status == SessionStatus::Hibernated {
            let mut info = info;
            // Existence only: the key (a keychain read) is never touched before the socket
            // answers. A launch that does not open is found at the wake.
            if registry.launches().exists(&info.id) {
                session::start_hibernated(registry.clone(), info);
            } else {
                // It could never wake: Reopen starts it again from its conversation.
                tracing::warn!("session {} has no stored launch; it is exited", info.id);
                mark_exited(&mut info, None);
                registry.update(info);
            }
        } else {
            registry.update(info);
        }
    }
    registry.prune_hibernated(agent::now_ms());
    Ok(pending)
}

/// What `sessions/*.sock` can tell without the state file: the ids. Command, title and size are
/// lost; the sessions themselves are found again.
fn probe_holders(registry: &Registry) -> Vec<SessionInfo> {
    let Ok(entries) = fs::read_dir(registry.home.sessions()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        found.push(SessionInfo {
            id: id.to_string(),
            cmd: Vec::new(),
            cwd: String::new(),
            title: None,
            status: SessionStatus::Running,
            exit_code: None,
            holder_pid: None,
            cols: 80,
            rows: 24,
            project: None,
            group: None,
            agent: Default::default(),
        });
    }
    found
}

/// Reattaches every holder at the same time, in the background: one slow holder delays only
/// its own session, never the socket or the others.
fn reattach_all(registry: &Arc<Registry>, pending: Vec<Pending>) {
    for Pending { info, probed } in pending {
        let registry = registry.clone();
        tokio::spawn(async move {
            let id = info.id.clone();
            let outcome = reattach(&registry, info).await;
            if let Err(mut info) = outcome {
                if probed {
                    // A socket nobody answers on is a leftover, not a session.
                    registry.forget_announced(&id);
                    registry.settle();
                    return;
                }
                let opens = registry.launches().exists(&id);
                let resumable = agent::agent_of(info.agent.name.as_deref()).is_some();
                if can_hibernate(&info, opens, resumable) {
                    // The process died with the machine (or the holder was killed): the
                    // conversation is still there, so the session sleeps instead of ending.
                    mark_hibernated(&mut info, agent::now_ms());
                    session::start_hibernated(registry.clone(), info);
                } else {
                    mark_exited(&mut info, None);
                    registry.update(info);
                }
            }
            registry.announce_updated(&id);
            registry.settle();
        });
    }
}

/// Connects to the session's holder and starts its actor. On failure the info comes back.
async fn reattach(
    registry: &Arc<Registry>,
    info: SessionInfo,
) -> std::result::Result<(), SessionInfo> {
    let sock = registry.home.sessions().join(format!("{}.sock", info.id));
    let attached = async {
        let mut conn = holder::HolderConn::connect(&sock).await?;
        let mut screen = Screen::new(info.rows, info.cols);
        let seq = conn.attach_and_replay(&mut screen).await?;
        Ok::<_, Error>((conn, screen, seq))
    }
    .await;
    match attached {
        Ok((conn, screen, seq)) => {
            session::start(registry.clone(), info, conn, screen, seq, sock, false);
            Ok(())
        }
        Err(e) => {
            tracing::warn!("session {} lost its holder: {e}", info.id);
            Err(info)
        }
    }
}

async fn status(home: Home) -> Result<String> {
    let stream = UnixStream::connect(home.socket()).await?;
    let (read, mut write) = stream.into_split();
    let mut reader = framed::FrameReader::new(read);
    let hello = Request::new(
        1,
        method::HELLO,
        Hello {
            protocol: PROTOCOL_VERSION,
            client: "status".into(),
            role: None,
        },
    );
    framed::write_frame(&mut write, &hello.frame()).await?;
    framed::write_frame(
        &mut write,
        &Request::new(2, method::SESSION_LIST, ()).frame(),
    )
    .await?;
    let mut daemon = String::new();
    while let Some(frame) = reader.next().await? {
        let Frame::Json(text) = frame else { continue };
        let Ok(Message::Response(response)) = Message::parse(&text) else {
            continue;
        };
        let result = response.result.unwrap_or_default();
        if response.id == 1 {
            daemon = result["daemon"].as_str().unwrap_or_default().to_string();
        } else {
            let sessions: Vec<SessionInfo> = serde_json::from_value(result)?;
            let running = sessions
                .iter()
                .filter(|s| s.status == SessionStatus::Running)
                .count();
            return Ok(format!(
                "sushiai daemon {daemon}: {} sessions, {running} running",
                sessions.len()
            ));
        }
    }
    Err(Error::Holder("daemon closed the connection".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[tokio::test]
    async fn the_socket_is_never_group_or_world_accessible_even_under_umask_zero() {
        let dir = tempfile::Builder::new()
            .prefix("sb")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let path = dir.path().join("s.sock");
        // SAFETY: umask(2) has no preconditions; restored below.
        let old = unsafe { libc::umask(0) };
        let bound = bind_private(&path);
        // SAFETY: as above.
        let restored = unsafe { libc::umask(old) };
        let _listener = bound.expect("bind");
        assert_eq!(restored, 0, "bind_private left the umask changed");
        let mode = fs::metadata(&path).expect("meta").mode();
        assert_eq!(mode & 0o077, 0, "socket mode {mode:o} before any chmod");
    }
}
