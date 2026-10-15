use crate::harness::cancel::CancelToken;
use crate::harness::environment::augmented_path;
use crate::harness::guard::Guard;
use crate::harness::guard::Trip;
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;

/// `codex --version`, asked once per binary per daemon start.
pub(super) async fn codex_version(
    bin: &str,
    cancel: &CancelToken,
    guard: &mut Guard,
) -> Result<Option<String>, Trip> {
    use tokio::io::AsyncReadExt;

    static CACHE: OnceLock<StdMutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().unwrap().get(bin) {
        return Ok(known.clone());
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--version")
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(None),
    };
    let pgid = child.id().map(|pid| pid as i32);
    let mut stdout = child.stdout.take().expect("piped stdout");
    let result = {
        let output = async {
            let mut bytes = Vec::new();
            let (status, read) = tokio::join!(child.wait(), stdout.read_to_end(&mut bytes));
            (status, read, bytes)
        };
        tokio::pin!(output);
        loop {
            tokio::select! {
                output = &mut output => break Ok(Some(output)),
                _ = cancel.cancelled() => break Ok(None),
                _ = tokio::time::sleep_until(guard.deadline()), if guard.armed() => {
                    if let Some(trip) = guard.stall_expired() {
                        break Err(trip);
                    }
                }
            }
        }
    };
    let (status, read, bytes) = match result {
        Ok(Some(output)) => output,
        stopped => {
            crate::procs::kill_group(pgid, &mut child).await;
            return stopped.map(|_| None);
        }
    };
    let version = status
        .ok()
        .filter(|status| status.success() && read.is_ok())
        .map(|_| String::from_utf8_lossy(&bytes).trim().to_string())
        .filter(|v| !v.is_empty());
    cache
        .lock()
        .unwrap()
        .insert(bin.to_string(), version.clone());
    Ok(version)
}
