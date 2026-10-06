use std::io::stderr;
use std::path::PathBuf;

mod hook;
mod hooks_file;

use anyhow::{bail, Context, Result};

const USAGE: &str = "usage: sushiai daemon | status | hook EVENT | hooks install|uninstall | hold --id ID --dir DIR --cols N --rows N --cwd DIR -- CMD [ARGS...]";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if command.as_deref() == Some("hook") {
        // An agent runs this for every hook: no logging, never a failure.
        hook::run(args);
        return Ok(());
    }
    tracing_subscriber::fmt().with_writer(stderr).init();
    match command.as_deref() {
        Some("hooks") => hooks_file::run(args.next().as_deref())?,
        Some("daemon") => sushiai_daemon::run_blocking(sushiai_daemon::Home::from_env())?,
        Some("status") => println!(
            "{}",
            sushiai_daemon::status_blocking(sushiai_daemon::Home::from_env())?
        ),
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
