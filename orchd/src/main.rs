//! `orchd serve --data <dir> [--socket <path>]` (also the default with no
//! subcommand), `orchd hook stop --socket <path> --token <t>`,
//! `orchd hook rtk` (no socket or token -- it never contacts the daemon),
//! `orchd mcp --data <dir> [--socket <path>]`, `orchd ab --data <dir> [--eval <set>]`, `orchd failures --data <dir>`, and
//! `orchd eval run --data <dir> --socket <sock> ...` (see `eval.rs`), and
//! `orchd evolve --data <dir> [--socket <sock>] [--adopt <id>]` (see `evolve.rs`).

mod ab;
mod brief;
mod classify;
mod engine;
mod eval;
mod evolve;
mod git;
mod harness;
mod hook;
mod loop_detect;
mod mcp;
mod model;
mod protocol;
mod store;
mod timeline;

use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rt = tokio::runtime::Runtime::new().expect("failed to start the tokio runtime");
    let code = rt.block_on(run(args));
    std::process::exit(code);
}

async fn run(args: Vec<String>) -> i32 {
    if args.len() >= 2 && args[1] == "hook" {
        return run_hook(&args[2..]).await;
    }
    if args.len() >= 2 && args[1] == "ab" {
        return ab::run(&args[2..]);
    }
    if args.len() >= 2 && args[1] == "failures" {
        return timeline::run(&args[2..]);
    }
    if args.len() >= 2 && args[1] == "eval" {
        return eval::run(&args[2..]);
    }
    if args.len() >= 2 && args[1] == "evolve" {
        return evolve::run(&args[2..]);
    }
    if args.len() >= 2 && args[1] == "mcp" {
        return mcp::run(&args[2..]);
    }
    let sub_args: &[String] = if args.len() >= 2 && args[1] == "serve" {
        &args[2..]
    } else {
        &args[1..]
    };
    run_serve(sub_args).await
}

async fn run_hook(args: &[String]) -> i32 {
    let kind = args.first().map(String::as_str);
    if !matches!(kind, Some("stop" | "rtk")) {
        println!("{{}}");
        return 0;
    }
    if kind == Some("rtk") {
        match run_rtk_hook().await {
            Some(v) => println!("{v}"),
            None => println!("{{}}"),
        }
        return 0;
    }
    let mut socket: Option<String> = None;
    let mut token: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" if i + 1 < args.len() => {
                socket = Some(args[i + 1].clone());
                i += 2;
            }
            "--token" if i + 1 < args.len() => {
                token = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    match run_hook_inner(socket, token).await {
        Some(v) => println!("{v}"),
        None => println!("{{}}"),
    }
    0
}

/// Reads the hook JSON on stdin, sends `hook.stop`, prints the result.
/// Fails open (`None` -> caller prints `{}`) on any error: missing flags,
/// bad stdin, no daemon listening, malformed response.
async fn run_hook_inner(socket: Option<String>, token: Option<String>) -> Option<String> {
    let socket = socket?;
    let token = token?;
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).ok()?;
    let payload: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
    let result = crate::protocol::client_request(
        Path::new(&socket),
        "hook.stop",
        serde_json::json!({"token": token, "payload": payload}),
    )
    .await
    .ok()?;
    Some(result.to_string())
}

/// `rtk rewrite <command>`'s own timeout: long enough for a real rewrite, far
/// short of Claude Code's hook-level timeout (`harness::RTK_HOOK_TIMEOUT_SECS`)
/// so the hook always gets to answer for itself.
const RTK_REWRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Test-only override (milliseconds) so a loaded machine can widen the window
/// without changing what ships; unset, [`RTK_REWRITE_TIMEOUT`] applies.
fn rtk_rewrite_timeout() -> std::time::Duration {
    std::env::var("ORCHD_RTK_REWRITE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(RTK_REWRITE_TIMEOUT)
}

/// `orchd hook rtk`: the PreToolUse payload on stdin (a Bash tool call)
/// becomes an `rtk rewrite <command>` call, with no socket or token -- this
/// hook never contacts the daemon. Fails open (`None` -> caller prints
/// `{}`) on any error, same as the other hooks: bad stdin, rtk missing from
/// PATH, a non-zero exit, a timeout, an unchanged command, or a rewrite
/// that touches a denied git prefix.
async fn run_rtk_hook() -> Option<String> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).ok()?;
    let payload: serde_json::Value = serde_json::from_str(&input).ok()?;
    let command = hook::rtk_hook_command(&payload)?;
    if hook::touches_denied_bash_command(&command) {
        return None;
    }
    let (code, stdout) = run_rtk_rewrite(&command).await;
    let tool_input = payload.get("tool_input")?;
    hook::rtk_rewrite_output(tool_input, &command, code, &stdout).map(|v| v.to_string())
}

/// Runs `rtk rewrite <command>`, killing it if it outlives
/// [`RTK_REWRITE_TIMEOUT`] (`kill_on_drop` fires when the timed-out future
/// drops the still-owned child). `(None, "")` when rtk is missing from PATH
/// or times out; otherwise its exit code (see `hook::rtk_rewrite_output`).
async fn run_rtk_rewrite(command: &str) -> (Option<i32>, String) {
    let child = tokio::process::Command::new("rtk")
        .arg("rewrite")
        .arg(command)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(child) = child else {
        return (None, String::new());
    };
    match tokio::time::timeout(rtk_rewrite_timeout(), child.wait_with_output()).await {
        Ok(Ok(out)) => (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).to_string(),
        ),
        Ok(Err(_)) | Err(_) => (None, String::new()),
    }
}

async fn run_serve(args: &[String]) -> i32 {
    let mut data_dir_arg: Option<String> = None;
    let mut socket_arg: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--data" if i + 1 < args.len() => {
                data_dir_arg = Some(args[i + 1].clone());
                i += 2;
            }
            "--socket" if i + 1 < args.len() => {
                socket_arg = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }

    let Some(data_dir_arg) = data_dir_arg else {
        eprintln!("orchd serve: --data <dir> is required");
        return 2;
    };
    let data_dir = PathBuf::from(data_dir_arg);
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("orchd: cannot create data dir {}: {e}", data_dir.display());
        return 1;
    }
    let socket_path = socket_arg
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("orchd.sock"));

    let pidfile = data_dir.join("orchd.pid");
    if let Err(msg) = acquire_singleton(&pidfile) {
        eprintln!("orchd: {msg}");
        return 1;
    }
    if let Err(e) = std::fs::write(&pidfile, std::process::id().to_string()) {
        eprintln!("orchd: cannot write pidfile: {e}");
        return 1;
    }

    let orchd_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "orchd".to_string());

    let app = match engine::App::new(data_dir.clone(), socket_path.clone(), orchd_path) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("orchd: failed to start: {e}");
            return 1;
        }
    };
    if let Err(e) = app.recover_on_start() {
        eprintln!("orchd: restart recovery failed: {e}");
    }

    let dispatcher: std::sync::Arc<dyn protocol::Dispatcher> = app.clone();
    let serve_shutdown_rx = app.subscribe_shutdown();
    let socket_path2 = socket_path.clone();
    let mut serve_task =
        tokio::spawn(
            async move { protocol::serve(&socket_path2, dispatcher, serve_shutdown_rx).await },
        );

    let mut shutdown_rx = app.subscribe_shutdown();
    let mut already_joined = false;
    let exit_code;
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        exit_code = tokio::select! {
            _ = sigterm.recv() => { app.shutdown(); 0 }
            _ = shutdown_rx.recv() => { 0 }
            // The socket server can also end on its own -- most notably
            // when `UnixListener::bind` fails (e.g. the socket path is too
            // long). Without this branch the daemon would sit here forever
            // waiting for a shutdown signal nobody will ever send.
            joined = &mut serve_task => {
                already_joined = true;
                report_serve_outcome(&socket_path, joined)
            }
        };
    }
    #[cfg(not(unix))]
    {
        exit_code = tokio::select! {
            _ = shutdown_rx.recv() => { 0 }
            joined = &mut serve_task => {
                already_joined = true;
                report_serve_outcome(&socket_path, joined)
            }
        };
    }

    if !already_joined {
        let _ = serve_task.await;
    }
    // `app.shutdown()` only *starts* cancelling every running task loop,
    // chat turn and audit (each one still has to reach its own
    // `cancel.cancelled()` check and `killpg` its child); give that a
    // bounded window to actually finish before the process exits out from
    // under them.
    let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while (app.any_task_loop_running() || app.any_chat_turn_running() || app.any_audit_running())
        && tokio::time::Instant::now() < drain_deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let _ = std::fs::remove_file(&pidfile);
    exit_code
}

fn report_serve_outcome(
    socket_path: &Path,
    joined: Result<std::io::Result<()>, tokio::task::JoinError>,
) -> i32 {
    match joined {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            eprintln!("orchd: failed to listen on {}: {e}", socket_path.display());
            1
        }
        Err(e) => {
            eprintln!("orchd: socket server task panicked: {e}");
            1
        }
    }
}

/// Pidfile lock in the data dir: if it names a process that is (a) alive
/// and (b) actually looks like an `orchd`, refuse to start a second
/// instance. This does not depend on the socket path at all -- two
/// invocations can legitimately choose different `--socket` paths for the
/// same `--data` dir (Electron falls back to a `$TMPDIR` path when the
/// natural one is too long), so pinging "the" socket is not a reliable
/// singleton check. A stale pidfile (dead process, or a pid recycled by an
/// unrelated program) is ignored; the socket file itself is removed by
/// `protocol::serve` before binding.
fn acquire_singleton(pidfile: &Path) -> Result<(), String> {
    let Ok(contents) = std::fs::read_to_string(pidfile) else {
        return Ok(());
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        return Ok(());
    };
    if pid <= 0 {
        return Ok(());
    }
    #[cfg(unix)]
    let alive = unsafe { libc::kill(pid, 0) == 0 };
    #[cfg(not(unix))]
    let alive = false;
    if !alive {
        return Ok(());
    }
    if !process_looks_like_orchd(pid) {
        return Ok(());
    }
    Err(format!(
        "another orchd is already running (pid {pid}); not starting a second instance"
    ))
}

/// Best-effort identity check so a pid recycled by an unrelated program
/// after `orchd` died doesn't wrongly block a new daemon forever. If `ps`
/// itself is unavailable or unparseable, fail closed (treat it as an
/// `orchd`) rather than risk two daemons racing over the same data dir.
#[cfg(unix)]
fn process_looks_like_orchd(pid: i32) -> bool {
    match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .trim()
            .contains("orchd"),
        _ => true,
    }
}

#[cfg(not(unix))]
fn process_looks_like_orchd(_pid: i32) -> bool {
    true
}
