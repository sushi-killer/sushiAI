//! `orchd serve --data <dir> [--socket <path>]` (also the default with no
//! subcommand), `orchd hook stop|edit --socket <path> --token <t>`,
//! `orchd mcp --data <dir> [--socket <path>]`, `orchd ab --data <dir> [--eval <set>]`, `orchd failures --data <dir>`, and
//! `orchd gc --data <dir> --socket <sock> [--dry-run]`,
//! `orchd eval run --data <dir> --socket <sock> ...` (see `eval.rs`), and
//! `orchd costs [backfill] --data <dir> [--since <days>] [--by stage,model] [--json]` (see `costs.rs`), and
//! `orchd evolve --data <dir> [--socket <sock>] [--adopt <id>]` (see `evolve.rs`).

mod ab;
mod brief;
mod classify;
mod costs;
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
mod report;
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
    if args.len() >= 2 && args[1] == "costs" {
        return costs::run(&args[2..]);
    }
    if args.len() >= 2 && args[1] == "evolve" {
        return evolve::run(&args[2..]);
    }
    if args.len() >= 2 && args[1] == "gc" {
        return run_gc(&args[2..]);
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

/// `orchd gc --data <dir> --socket <sock> [--dry-run]`: asks the running
/// daemon to remove the worktrees its tasks no longer need and prints what
/// it did (`worktrees.gc`).
fn run_gc(args: &[String]) -> i32 {
    let (mut data, mut socket, mut dry_run) = (None, None, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--data" if i + 1 < args.len() => {
                data = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--socket" if i + 1 < args.len() => {
                socket = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
    let (Some(data), Some(socket)) = (data, socket) else {
        eprintln!("orchd gc: usage: orchd gc --data <dir> --socket <sock> [--dry-run]");
        return 2;
    };
    let result = mcp::read_control_token(&data).and_then(|token| {
        mcp::call_orchd(
            &socket,
            &token,
            "worktrees.gc",
            serde_json::json!({"dryRun": dry_run}),
        )
    });
    match result {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("orchd gc: {e}");
            1
        }
    }
}

async fn run_hook(args: &[String]) -> i32 {
    let method = match args.first().map(String::as_str) {
        Some("stop") => "hook.stop",
        Some("edit") => "hook.edit",
        _ => {
            println!("{{}}");
            return 0;
        }
    };
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
    match run_hook_inner(method, socket, token).await {
        Some(v) => println!("{v}"),
        None => println!("{{}}"),
    }
    0
}

/// Reads the hook JSON on stdin, sends `method`, prints the result.
/// Fails open (`None` -> caller prints `{}`) on any error: missing flags,
/// bad stdin, no daemon listening, malformed response.
async fn run_hook_inner(
    method: &str,
    socket: Option<String>,
    token: Option<String>,
) -> Option<String> {
    let socket = socket?;
    let token = token?;
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).ok()?;
    let payload: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
    let result = crate::protocol::client_request(
        Path::new(&socket),
        method,
        serde_json::json!({"token": token, "payload": payload}),
    )
    .await
    .ok()?;
    Some(result.to_string())
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
    let data_identity = dir_identity(&data_dir);
    let mut already_joined = false;
    let exit_code;
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        exit_code = tokio::select! {
            _ = sigterm.recv() => { app.shutdown(); 0 }
            _ = shutdown_rx.recv() => { 0 }
            _ = data_dir_gone(&data_dir, data_identity) => { app.shutdown(); 0 }
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
            _ = data_dir_gone(&data_dir, data_identity) => { app.shutdown(); 0 }
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

/// Identity of the data dir (device, inode) on unix; existence elsewhere.
#[cfg(unix)]
fn dir_identity(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn dir_identity(dir: &Path) -> Option<(u64, u64)> {
    dir.is_dir().then_some((0, 0))
}

/// Resolves once the data dir is gone or names a different directory (one
/// recreated by a late write), polling about once a second. A test-launched
/// app's profile is deleted after it quits; this keeps a crashed app from
/// leaving its daemon behind.
async fn data_dir_gone(dir: &Path, identity: Option<(u64, u64)>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if dir_identity(dir) != identity {
            return;
        }
    }
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
