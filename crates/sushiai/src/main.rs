use std::io::stderr;
use std::path::PathBuf;

mod agents_cli;
mod daemon_client;
mod hook;
mod hooks_file;
mod mcp;
mod open;
mod orch_cli;
mod orch_module;
mod proc;
mod routines_cli;
mod routines_module;
mod skills_cli;

use anyhow::{bail, Context, Result};
use tracing_subscriber::filter::LevelFilter;

mod proxy;

const USAGE: &str = "usage: sushiai daemon | status | shutdown | proc PID | proxy | mcp [--task ID | --read-only | --routines] | orch hook|ab|eval|costs|failures|evolve|gc|register|unregister | hook EVENT | open TARGET PATH | hooks install|uninstall | projects [show ID] | hosts | agents catalog [--json] | skills [--project DIR] [--json] [--catalog] | skills usage --index [--json] | skills install|remove NAME | routines [show ID|help] | routine save|validate|dry-run | ask TEXT | propose pr|commit|branch | hold --id ID --dir DIR --cols N --rows N --cwd DIR -- CMD [ARGS...]";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if command.as_deref() == Some("hook") {
        // An agent runs this for every hook: no logging, never a failure.
        hook::run(args);
        return Ok(());
    }
    if command.as_deref() == Some("open") {
        // Run by an agent: a missing session or daemon is a note on stderr, never a failure.
        open::run(args);
        return Ok(());
    }
    // WARN keeps daemon.log small; `SUSHIAI_LOG=info|debug|trace|off` changes it.
    let level = std::env::var("SUSHIAI_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::WARN);
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(stderr)
        .init();
    match command.as_deref() {
        Some("hooks") => hooks_file::run(args.next().as_deref())?,
        Some("daemon") => {
            leave_background_priority();
            let home = sushiai_daemon::Home::from_env();
            let mut modules = orch_module::slots(&home);
            modules.extend(routines_module::slots(&home));
            sushiai_daemon::run_blocking(home, modules)?;
        }
        Some("status") => println!(
            "{}",
            sushiai_daemon::status_blocking(sushiai_daemon::Home::from_env())?
        ),
        Some("proc") => proc::run(args)?,
        Some("shutdown") => {
            let mut conn =
                daemon_client::Conn::direct(&sushiai_daemon::Home::from_env().socket(), "shutdown")
                    .map_err(anyhow::Error::msg)?;
            conn.call(
                "daemon.shutdown",
                serde_json::json!({"closeSessions": true}),
            )
            .map_err(|e| anyhow::Error::msg(e.message()))?;
        }
        Some(command @ ("projects" | "hosts" | "routines" | "routine" | "ask" | "propose")) => {
            std::process::exit(routines_cli::run(command, args))
        }
        Some("agents") => std::process::exit(agents_cli::run(args)),
        Some("skills") => std::process::exit(skills_cli::run(args)),
        Some("mcp") => std::process::exit(mcp::run(args)),
        Some("orch") => std::process::exit(orch_cli::run(args)),
        Some("proxy") => proxy::run(&sushiai_daemon::Home::from_env())?,
        Some("--version") => println!("sushiai {}", env!("CARGO_PKG_VERSION")),
        Some("hold") => {
            // A startup failure goes to stderr, where the daemon reads it; once the holder
            // runs, stderr is closed, which tells the daemon all is well.
            let running = sushiai_hold::start(hold_config(args)?)?;
            sushiai_hold::detach_stderr();
            running.serve()?;
        }
        _ => bail!(USAGE),
    }
    Ok(())
}

/// A daemon started by a background launchd job (the link) inherits macOS
/// background priority, and so does every agent and check it runs: they get
/// only the efficiency cores and run several times slower under load.
fn leave_background_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: setpriority(2) on the calling process touches no memory.
    if unsafe { libc::setpriority(libc::PRIO_DARWIN_PROCESS, 0, 0) } != 0 {
        eprintln!(
            "cannot leave background priority: {}",
            std::io::Error::last_os_error()
        );
    }
}

fn hold_config(mut args: impl Iterator<Item = String>) -> Result<sushiai_hold::Config> {
    let (mut id, mut dir, mut cols, mut rows, mut cwd) = (None, None, None, None, None);
    let mut cmd = Vec::new();
    while let Some(flag) = args.next() {
        if flag == "--" {
            cmd.extend(args.by_ref());
            break;
        }
        let value = args
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--id" => id = Some(value),
            "--dir" => dir = Some(PathBuf::from(value)),
            "--cols" => cols = Some(value.parse()?),
            "--rows" => rows = Some(value.parse()?),
            "--cwd" => cwd = Some(PathBuf::from(value)),
            _ => bail!("unknown flag {flag}"),
        }
    }
    Ok(sushiai_hold::Config {
        id: id.context("--id is required")?,
        dir: dir.context("--dir is required")?,
        cols: cols.context("--cols is required")?,
        rows: rows.context("--rows is required")?,
        cwd: cwd.context("--cwd is required")?,
        cmd,
    })
}
