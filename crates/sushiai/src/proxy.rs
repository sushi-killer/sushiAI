//! `sushiai proxy`: relays stdin/stdout to the daemon socket, byte for byte. It never parses
//! frames. If nobody answers on the socket it starts a detached daemon first.
//!
//! Exit codes: 0 after stdin ended and the daemon's last bytes were drained,
//! `connector::DAEMON_DIED` (2) when the daemon closed the connection first, 1 for every
//! other failure.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use sushiai_protocol::connector;

const START_WAIT: Duration = Duration::from_secs(15);
/// After stdin ends and the socket is half-closed: at most this long for the daemon to send
/// what is queued and close.
const DRAIN_WAIT: Duration = Duration::from_secs(5);
const PATH_WAIT: Duration = Duration::from_secs(3);
const FALLBACK_PATH: &str = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin";
const ENV_ALLOW: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "TMPDIR",
    "SUSHIAI_LOG",
];

pub fn run(home: &sushiai_daemon::Home) -> Result<()> {
    let socket = home.socket();
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            let child = start_daemon(&socket)?;
            wait_for(&socket, child)?
        }
        Err(e) => return Err(e).with_context(|| format!("cannot connect to {}", socket.display())),
    };
    relay(stream)
}

fn home_dir(socket: &Path) -> PathBuf {
    socket.parent().unwrap_or(Path::new(".")).to_path_buf()
}

fn log_path(socket: &Path) -> PathBuf {
    home_dir(socket).join("daemon.log")
}

/// The login shell's PATH, so a daemon started from a bare ssh command still finds `claude`.
fn login_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let fallback = format!("{FALLBACK_PATH}:{home}/.local/bin");
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    // The proxy's own PATH (often a bare ssh one) must not seed the login shell's.
    let spawned = Command::new(shell)
        .args(["-lc", "printf %s \"$PATH\""])
        .env_remove("PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return fallback;
    };
    let Some(mut stdout) = child.stdout.take() else {
        return fallback;
    };
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let result = rx.recv_timeout(PATH_WAIT);
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    // A login shell may print a banner first; PATH is the last line.
    match result
        .ok()
        .and_then(|t| t.lines().last().map(str::to_string))
    {
        Some(path) if !path.trim().is_empty() => path,
        _ => fallback,
    }
}

/// Starts `sushiai daemon` in its own session. Its environment is an allowlist: nothing from
/// the ssh connection (agent socket, client address) leaks into sessions.
fn start_daemon(socket: &Path) -> Result<Child> {
    let dir = home_dir(socket);
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_path(socket))
        .context("cannot open daemon.log")?;
    let installed = dir.join("bin").join("sushiai");
    let exe = if installed.exists() {
        installed
    } else {
        std::env::current_exe().context("cannot find the sushiai binary")?
    };
    let mut command = Command::new(exe);
    command.arg("daemon").env_clear();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if ENV_ALLOW.contains(&name.as_ref()) || name.starts_with("LC_") {
            command.env(key, value);
        }
    }
    command
        .env("SUSHIAI_HOME", &dir)
        .env("PATH", login_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // SAFETY: setsid(2) is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().context("cannot start the daemon")
}

/// The last lines of daemon.log, for an error message.
fn log_tail(socket: &Path) -> String {
    let text = fs::read_to_string(log_path(socket)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    tail.chars().take(2000).collect()
}

/// Waits for the daemon's socket. A daemon that exits is a failure unless it lost the race
/// to another daemon, in which case the other one's socket is coming.
fn wait_for(socket: &Path, child: Child) -> Result<UnixStream> {
    let mut child = Some(child);
    let deadline = Instant::now() + START_WAIT;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => {
                if let Some(mut child) = child {
                    std::thread::spawn(move || child.wait());
                }
                return Ok(stream);
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {}
            Err(e) => return Err(e).context("cannot connect to the daemon"),
        }
        if let Some(running) = child.as_mut() {
            if let Some(status) = running.try_wait().context("cannot poll the daemon")? {
                let tail = log_tail(socket);
                if !tail.contains("is already running") {
                    bail!("the daemon exited ({status}):\n{tail}");
                }
                child = None;
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "the daemon did not answer on {} within 15 s:\n{}",
                socket.display(),
                log_tail(socket)
            );
        }
        sleep(Duration::from_millis(50));
    }
}

enum Event {
    Data,
    StdinEnded,
    DaemonClosed,
    Failed(String),
}

fn failure(message: &str) -> i32 {
    eprintln!("sushiai proxy: {message}");
    1
}

/// Copies both ways. The two copy threads only report; this function decides the exit code.
fn relay(stream: UnixStream) -> ! {
    let (tx, rx) = channel();
    let (reader, closer) = match (stream.try_clone(), stream.try_clone()) {
        (Ok(reader), Ok(closer)) => (reader, closer),
        (Err(e), _) | (_, Err(e)) => {
            std::process::exit(failure(&format!("cannot clone the socket: {e}")))
        }
    };
    spawn_output(reader, tx.clone());
    spawn_input(stream, tx);
    let code = loop {
        match rx.recv() {
            Ok(Event::Data) => {}
            Ok(Event::StdinEnded) => break drain(&closer, &rx),
            Ok(Event::DaemonClosed) => {
                eprintln!("sushiai proxy: the daemon closed the connection");
                break connector::DAEMON_DIED;
            }
            Ok(Event::Failed(message)) => break failure(&message),
            Err(_) => break 1,
        }
    };
    std::process::exit(code);
}

/// Stdin has ended: half-close the socket. The daemon answers what it has read, then closes
/// its side; that close ends the proxy.
fn drain(socket: &UnixStream, rx: &Receiver<Event>) -> i32 {
    let _ = socket.shutdown(std::net::Shutdown::Write);
    let deadline = Instant::now() + DRAIN_WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Event::Data | Event::StdinEnded) => {}
            Ok(Event::Failed(message)) => return failure(&message),
            Ok(Event::DaemonClosed) | Err(_) => return 0,
        }
    }
}

/// Daemon to stdout. Every chunk is flushed: stdout is line-buffered and the data is binary.
fn spawn_output(mut socket: UnixStream, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let mut out = std::io::stdout().lock();
        let mut buf = [0u8; 64 * 1024];
        let event = loop {
            match socket.read(&mut buf) {
                Ok(0) => break Event::DaemonClosed,
                Ok(n) => {
                    if let Err(e) = out.write_all(&buf[..n]).and_then(|()| out.flush()) {
                        break Event::Failed(format!("write to stdout failed: {e}"));
                    }
                    let _ = tx.send(Event::Data);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => break Event::Failed(format!("read from the daemon failed: {e}")),
            }
        };
        let _ = tx.send(event);
    });
}

/// Stdin to daemon. Reports the end of stdin; the relay decides when to half-close.
fn spawn_input(mut socket: UnixStream, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 64 * 1024];
        let event = loop {
            match stdin.read(&mut buf) {
                Ok(0) => break Event::StdinEnded,
                Ok(n) => {
                    if let Err(e) = socket.write_all(&buf[..n]) {
                        break Event::Failed(format!("write to the daemon failed: {e}"));
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => break Event::Failed(format!("read from stdin failed: {e}")),
            }
        };
        let _ = tx.send(event);
    });
}
