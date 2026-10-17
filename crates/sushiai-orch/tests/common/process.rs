use super::harness::{fake_codex_script, unstubbed_claude_script};
use super::native_sandbox::fit_sandbox;
use super::polling::{deadline, poll_deadline, wait_until};
use super::protocol::{raw_request_with_params, request_on, stop_daemon, wait_for_socket};
use super::sandbox::native_sandbox_unavailable;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::{Mutex, OnceLock};

pub struct Daemon {
    pub child: DaemonChild,
    pub socket: PathBuf,
    /// The sushiAI home the daemon runs in; the orchestrator lives in `<home>/orchestrator`.
    pub home: tempfile::TempDir,
    data: PathBuf,
}

/// A `sushiai daemon` child that dies with its owner. Every daemon a test spawns -- through
/// `Daemon` or `spawn_daemon` -- is held in one, so a test that panics, or simply forgets to
/// shut its daemon down, cannot leave an orphan behind. Dropping reaps the process.
pub struct DaemonChild(Child);

pub(super) fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl std::ops::Deref for DaemonChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for DaemonChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// The `sushiai` binary the daemon runs as: `SUSHIAI_BIN`, else the workspace's own build,
/// brought up to date once per test process.
pub fn sushiai_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(bin) = std::env::var_os("SUSHIAI_BIN") {
            return PathBuf::from(bin);
        }
        let exe = std::env::current_exe().expect("test executable");
        let profile_dir = exe
            .parent()
            .and_then(Path::parent)
            .expect("target profile dir")
            .to_path_buf();
        let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        build
            .args(["build", "--quiet", "-p", "sushiai"])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            build.arg("--release");
        }
        assert!(
            build.status().is_ok_and(|s| s.success()),
            "cargo build -p sushiai failed"
        );
        profile_dir.join("sushiai")
    })
}

/// The daemon socket of a home.
pub fn socket_of(home: &Path) -> PathBuf {
    home.join("daemon.sock")
}

/// Starts `sushiai daemon` in `home` with the orchestrator module enabled, like the desktop
/// does. Its output goes to `<home>/daemon-test.log`.
pub fn spawn_daemon(home: &Path, extra_env: &[(&str, &str)]) -> DaemonChild {
    let user = home.join("user");
    std::fs::create_dir_all(user.join(".codex")).unwrap();
    std::fs::write(
        user.join(".gitconfig"),
        "[user]\n\tname = orch test\n\temail = orch-test@example.invalid\n",
    )
    .unwrap();
    std::fs::create_dir_all(home.join("modules")).unwrap();
    std::fs::write(home.join("modules/orch.enabled"), b"").unwrap();
    let log = std::fs::File::options()
        .create(true)
        .append(true)
        .open(home.join(DAEMON_LOG))
        .unwrap();
    let mut cmd = Command::new(sushiai_bin());
    cmd.arg("daemon")
        .env("SUSHIAI_HOME", home)
        .env("HOME", &user)
        .env("CODEX_HOME", user.join(".codex"))
        // An owner-set wrapper turns the compiler cache off: a test daemon must not start
        // or join the owner's sccache server (fixed port 4227).
        .env("RUSTC_WRAPPER", "")
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    // No test daemon reaches the real codex binary: a review routed to codex
    // (a no-verdict review is retried there) gets this prose stub, unless the
    // test names its own codex in `extra_env`, which is set after this.
    cmd.env("ORCHD_CODEX_BIN", fake_codex_script(home));
    // The same for claude: a test that reaches the claude harness without naming
    // its own stub fails loudly instead of calling the real model.
    cmd.env("ORCHD_CLAUDE_BIN", unstubbed_claude_script(home));
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // A cargo build in another test process renames its output over the binary for a moment
    // (nothing there), and a binary just written can still be open for writing
    // (`ExecutableFileBusy`): both clear on their own.
    let spawned = poll_deadline(|| match cmd.spawn() {
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ExecutableFileBusy
            ) =>
        {
            None
        }
        spawned => Some(spawned),
    })
    .expect("the sushiai binary stayed missing or busy until the deadline");
    let child = spawned.expect("failed to spawn sushiai daemon");
    spawned_pids()
        .lock()
        .unwrap()
        .insert(socket_of(home), child.id());
    DaemonChild(child)
}

/// The pid of the daemon last spawned for each socket, so `wait_for_socket` can notice a
/// daemon that died instead of waiting out the deadline.
pub(super) fn spawned_pids() -> &'static Mutex<std::collections::HashMap<PathBuf, u32>> {
    static PIDS: OnceLock<Mutex<std::collections::HashMap<PathBuf, u32>>> = OnceLock::new();
    PIDS.get_or_init(Default::default)
}

/// Whether the child `pid` has exited, without reaping it (its owner still waits on it).
pub(super) fn has_exited(pid: u32) -> bool {
    // SAFETY: waitid(2) with a zeroed siginfo_t it owns; WNOWAIT leaves the child waitable.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let r = libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        );
        r == 0 && info.si_signo != 0
    }
}

impl Daemon {
    pub fn spawn(extra_env: &[(&str, &str)]) -> Daemon {
        let home = tempfile::tempdir().unwrap();
        let socket = socket_of(home.path());
        let child = spawn_daemon(home.path(), extra_env);
        wait_for_socket(&socket);
        // The brief check is one more harness run: off, so tests that count
        // runs, cost or argv see only the runs they set up. Its own tests
        // turn it back on.
        {
            let mut settings = request_on(&socket, "settings.get", serde_json::json!({}));
            settings["briefCheckRoute"] = serde_json::json!("");
            // The plan review would launch a real codex: off, so no test
            // ever does. The plan gate's own tests name a claude route.
            settings["planReview"] = serde_json::json!("");
            // The answer policy answers routine questions itself; the other
            // tests wait for them. Its own tests turn it back on.
            settings["answerPolicy"] = serde_json::json!(false);
            // A done task opens its pull request on its own, which runs the PR
            // drafter; the pull request tests turn it back on.
            settings["prOnDoneOff"] = serde_json::json!(true);
            if native_sandbox_unavailable() {
                fit_sandbox(&mut settings);
            }
            request_on(
                &socket,
                "settings.set",
                serde_json::json!({"settings": settings}),
            );
        }
        let data = home.path().join("orchestrator");
        Daemon {
            child,
            socket,
            home,
            data,
        }
    }

    /// The orchestrator's own folder.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    pub fn request(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        request_on(&self.socket, method, params)
    }

    /// The `error` text of a request that is expected to fail.
    pub fn request_error(&self, method: &str, params: serde_json::Value) -> String {
        let v = raw_request_with_params(&self.socket, method, params);
        v["error"].to_string()
    }

    /// Stops the daemon and waits for it to exit, then starts a new one in the same home.
    pub fn restart(&mut self, extra_env: &[(&str, &str)]) {
        stop_daemon(&self.socket);
        wait_until("the daemon to exit before its restart", || {
            matches!(self.child.try_wait(), Ok(Some(_))).then_some(())
        });
        self.child = spawn_daemon(self.home.path(), extra_env);
        wait_for_socket(&self.socket);
    }

    /// Stops the daemon and waits until it has exited. At the deadline it kills and reaps the
    /// daemon, then panics.
    pub fn shutdown_and_wait(mut self) {
        // Drop still runs afterwards and finds the child already reaped.
        stop_daemon(&self.socket);
        if poll_deadline(|| self.child.try_wait().ok().flatten()).is_none() {
            kill_and_reap(&mut self.child);
            panic!("the daemon did not exit within {:?}", deadline());
        }
    }
}

impl Drop for Daemon {
    /// Stops the daemon and waits for its exit before the home is removed, so a late write by
    /// the daemon or its agents cannot make the removal fail with ENOTEMPTY. Never panics:
    /// this can run while a failed test unwinds.
    fn drop(&mut self) {
        // A failed test may leave a wedged daemon: kill it at once instead of waiting.
        if std::thread::panicking() {
            kill_and_reap(&mut self.child);
        } else {
            stop_daemon(&self.socket);
            if poll_deadline(|| self.child.try_wait().ok().flatten()).is_none() {
                kill_and_reap(&mut self.child);
            }
        }
        let path = self.home.path().to_path_buf();
        poll_deadline(|| match std::fs::remove_dir_all(&path) {
            Ok(()) => Some(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(()),
            Err(_) => None,
        });
    }
}

/// The file a daemon's output goes to, inside its home, so a request that gets no answer can
/// say what the daemon printed.
pub(super) const DAEMON_LOG: &str = "daemon-test.log";

/// Wait for a raw (non-`Daemon`) child to exit, returning its status and
/// everything it wrote to stderr. Used for the singleton/bind-failure tests
/// where the process under test is expected to exit quickly on its own.
pub fn wait_for_exit(mut child: DaemonChild) -> (ExitStatus, String) {
    let Some(status) = poll_deadline(|| child.try_wait().ok().flatten()) else {
        kill_and_reap(&mut child);
        panic!("the daemon did not exit within {:?}", deadline());
    };
    let mut stderr = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    (status, stderr)
}

/// `kill(pid, 0)`: true iff a process with this pid exists and is signalable by us.
pub fn is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}
