//! The sushiai daemon: serves clients on a unix socket, runs one actor per session and
//! reconnects to surviving holders after a restart.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod agent;
mod binlink;
mod error;
mod framed;
mod holder;
mod home;
mod registry;
mod server;
mod session;

use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sushiai_core::{mark_exited, Screen, StateError, StateFile};
use sushiai_protocol::{
    method, Frame, Hello, Message, Request, SessionInfo, SessionStatus, PROTOCOL_VERSION,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};

pub use agent::ASK_WAIT_SECS;
pub use binlink::{ensure_bin_link, Link};
pub use error::{Error, Result};
pub use home::Home;
use registry::Registry;

/// Runs the daemon until SIGTERM or SIGINT. Holders keep running.
pub fn run_blocking(home: Home) -> Result<()> {
    runtime()?.block_on(run(home))
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

async fn run(home: Home) -> Result<()> {
    home.ensure()?;
    // The lock lives as long as this function: one daemon per home, decided before any recovery.
    let _lock = lock_home(&home)?;
    match binlink::ensure_bin_link(home.dir()) {
        Ok(Link::Kept) => tracing::warn!("bin/sushiai exists and is not a symlink; left alone"),
        Ok(_) => {}
        Err(e) => tracing::warn!("cannot link bin/sushiai: {e}"),
    }
    let socket = home.socket();
    let registry = Arc::new(Registry::new(home.clone()));
    recover(&registry).await?;
    // The socket exists only once the sessions are back, so a client never sees a partial list.
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    tracing::info!("daemon listening on {}", socket.display());

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        () = server::serve(listener, registry.clone()) => {}
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    let _ = fs::remove_file(&socket);
    registry.flush();
    Ok(())
}

/// Takes an exclusive, non-blocking `flock` on `<home>/daemon.lock`.
fn lock_home(home: &Home) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(home.lock())?;
    // SAFETY: flock(2) on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Error::AlreadyRunning(home.socket().display().to_string()));
    }
    Ok(file)
}

/// Loads the state file and reattaches to every holder that still answers.
/// A newer schema stops the daemon. A corrupt file is set aside and the catalog is rebuilt
/// from the holder sockets.
async fn recover(registry: &Arc<Registry>) -> Result<()> {
    let path = registry.home.state();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    match StateFile::from_bytes(&bytes) {
        Ok(state) => {
            for info in state.sessions {
                if info.status == SessionStatus::Running {
                    if let Err(mut info) = reattach(registry, info).await {
                        mark_exited(&mut info, None);
                        registry.update(info);
                    }
                } else {
                    registry.update(info);
                }
            }
            Ok(())
        }
        Err(e @ StateError::UnsupportedSchema(_)) => Err(e.into()),
        Err(StateError::Invalid(e)) => {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let aside = path.with_extension(format!("json.corrupt-{ts}"));
            tracing::warn!("state file is corrupt ({e}); moved to {}", aside.display());
            fs::rename(&path, aside)?;
            probe_holders(registry).await;
            Ok(())
        }
    }
}

/// Rebuilds the catalog from `sessions/*.sock`. What the state file knew (command, title,
/// size) is lost; the sessions themselves are found again.
async fn probe_holders(registry: &Arc<Registry>) {
    let Ok(entries) = fs::read_dir(registry.home.sessions()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let info = SessionInfo {
            id: id.to_string(),
            cmd: Vec::new(),
            cwd: String::new(),
            title: None,
            status: SessionStatus::Running,
            exit_code: None,
            holder_pid: None,
            cols: 80,
            rows: 24,
            agent: Default::default(),
        };
        // A socket nobody answers on is a leftover, not a session.
        let _ = reattach(registry, info).await;
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
            session::start(registry.clone(), info, conn, screen, seq, sock);
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
