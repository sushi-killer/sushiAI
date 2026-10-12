use anyhow::{bail, Context, Result};

pub fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    let pid: i32 = args.next().context("usage: sushiai proc PID")?.parse()?;
    if pid <= 0 || args.next().is_some() {
        bail!("usage: sushiai proc PID");
    }
    println!(
        "{}",
        serde_json::to_string(&sushiai_daemon::process::info(pid))?
    );
    Ok(())
}
