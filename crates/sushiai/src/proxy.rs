//! `sushiai proxy`: relays stdin/stdout to the daemon socket, byte for byte. It never parses
//! frames. If nobody answers on the socket it starts a detached daemon first.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

const START_WAIT: Duration = Duration::from_secs(5);

pub fn run(home: &sushiai_daemon::Home) -> Result<()> {
    let socket = home.socket();
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(_) => {
            start_daemon()?;
            wait_for(&socket)?
        }
    };
    relay(stream)
}

/// Starts `sushiai daemon` in its own session with stdio on /dev/null.
fn start_daemon() -> Result<()> {
    let exe = std::env::current_exe().context("cannot find the sushiai binary")?;
    let mut command = Command::new(exe);
    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid(2) is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().context("cannot start the daemon")?;
    Ok(())
}

fn wait_for(socket: &Path) -> Result<UnixStream> {
    let deadline = Instant::now() + START_WAIT;
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            return Ok(stream);
        }
        if Instant::now() >= deadline {
            bail!(
                "the daemon did not answer on {} within 5 s",
                socket.display()
            );
        }
        sleep(Duration::from_millis(50));
    }
}

/// Copies both ways until either side closes. Returns when stdin ends; the output thread
/// exits the process when the daemon closes the socket.
fn relay(stream: UnixStream) -> Result<()> {
    let mut to_stdout = stream.try_clone().context("cannot clone the socket")?;
    std::thread::spawn(move || {
        let mut out = std::io::stdout().lock();
        let mut buf = [0u8; 64 * 1024];
        let code = loop {
            match to_stdout.read(&mut buf) {
                Ok(0) => break 0,
                Ok(n) => {
                    if out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
                        break 0;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    eprintln!("sushiai proxy: read from the daemon failed: {e}");
                    break 1;
                }
            }
        };
        std::process::exit(code);
    });

    let mut to_daemon = stream;
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => to_daemon
                .write_all(&buf[..n])
                .context("write to the daemon failed")?,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("read from stdin failed"),
        }
    }
}
