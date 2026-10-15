use super::error::RunError;
use super::launch::{create_private, PreparedCommand};
use crate::harness::cancel::CancelToken;
use crate::harness::guard::{Guard, Trip};
use crate::harness::process::group_alive;
use crate::harness::run_files::RunFiles;
use crate::harness::{RunOutcome, RunRequest};
use crate::task::Fingerprint;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};

type Source = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
type FeedStdin = Pin<Box<dyn Future<Output = ()> + Send>>;

pub(crate) enum ProcessEvent {
    Stdout(String),
    Stderr(String),
    Exit(std::io::Result<ExitStatus>),
    Cancelled,
    Tripped(Trip),
}

pub(crate) struct NativeRun {
    child: tokio::process::Child,
    pgid: Option<i32>,
    out_lines: Lines<BufReader<Source>>,
    err_lines: Lines<BufReader<Source>>,
    feed_stdin: FeedStdin,
    stdin_done: bool,
    stdout_done: bool,
    stderr_done: bool,
    tracked: bool,
    wrapper_gone: Arc<AtomicBool>,
    argv: Vec<String>,
    codex_version: Option<String>,
}

pub(crate) struct SpawnedRun {
    child: tokio::process::Child,
    pgid: Option<i32>,
    argv: Vec<String>,
    codex_version: Option<String>,
}

impl SpawnedRun {
    pub(crate) fn spawn(
        prepared: PreparedCommand,
        files: Option<&RunFiles>,
    ) -> Result<Self, RunError> {
        let PreparedCommand {
            mut command,
            argv,
            codex_version,
            binary,
        } = prepared;
        configure_stdio(&mut command, files)?;
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let child = command.spawn().map_err(|e| {
            let text = format!("{binary}: {e}");
            if e.kind() == std::io::ErrorKind::NotFound {
                RunError::NotFound(text)
            } else {
                RunError::Io(text)
            }
        })?;
        let pgid = child.id().map(|p| p as i32);
        Ok(Self {
            child,
            pgid,
            argv,
            codex_version,
        })
    }

    pub(crate) fn pgid(&self) -> Option<i32> {
        self.pgid
    }

    pub(crate) fn attach(mut self, files: Option<&RunFiles>, brief: &str) -> NativeRun {
        let wrapper_gone = Arc::new(AtomicBool::new(false));
        let stdin = self.child.stdin.take();
        let (stdout, stderr): (Source, Source) = if let Some(files) = files {
            (
                tail_file(
                    files.raw.clone(),
                    files.exit.clone(),
                    self.pgid.unwrap_or(0),
                    wrapper_gone.clone(),
                ),
                tail_file(
                    files.stderr.clone(),
                    files.exit.clone(),
                    self.pgid.unwrap_or(0),
                    wrapper_gone.clone(),
                ),
            )
        } else {
            (
                Box::new(self.child.stdout.take().expect("piped stdout")),
                Box::new(self.child.stderr.take().expect("piped stderr")),
            )
        };
        let brief = brief.as_bytes().to_vec();
        let feed_stdin = Box::pin(async move {
            if let Some(mut stdin) = stdin {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(&brief).await;
            }
        });
        NativeRun {
            child: self.child,
            pgid: self.pgid,
            out_lines: BufReader::new(stdout).lines(),
            err_lines: BufReader::new(stderr).lines(),
            feed_stdin,
            stdin_done: files.is_some(),
            stdout_done: false,
            stderr_done: false,
            tracked: files.is_some(),
            wrapper_gone,
            argv: self.argv,
            codex_version: self.codex_version,
        }
    }
}

fn configure_stdio(
    command: &mut tokio::process::Command,
    files: Option<&RunFiles>,
) -> Result<(), RunError> {
    if let Some(files) = files {
        match (
            std::fs::File::open(&files.stdin).map(std::process::Stdio::from),
            create_private(&files.raw).map(std::process::Stdio::from),
            create_private(&files.stderr).map(std::process::Stdio::from),
        ) {
            (Ok(i), Ok(o), Ok(e)) => command.stdin(i).stdout(o).stderr(e),
            _ => return Err(RunError::Io("could not open the run files".into())),
        };
    } else {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    Ok(())
}

impl NativeRun {
    pub(crate) async fn stop(&mut self) {
        crate::procs::kill_group(self.pgid, &mut self.child).await;
    }

    pub(crate) fn fingerprint(&self, outcome: &RunOutcome, req: &RunRequest<'_>) -> Fingerprint {
        super::fingerprint::fingerprint_of(outcome, req, &self.argv, self.codex_version.clone())
    }

    pub(crate) async fn next(&mut self, cancel: &CancelToken, guard: &mut Guard) -> ProcessEvent {
        loop {
            tokio::select! {
                _ = &mut self.feed_stdin, if !self.stdin_done => { self.stdin_done = true; }
                _ = cancel.cancelled() => return ProcessEvent::Cancelled,
                _ = tokio::time::sleep_until(guard.deadline()), if guard.armed() => {
                    if let Some(trip) = guard.stall_expired() { return ProcessEvent::Tripped(trip); }
                }
                _ = tokio::time::sleep(Duration::from_millis(200)),
                    if self.tracked && !self.wrapper_gone.load(Ordering::SeqCst) => {
                    if matches!(self.child.try_wait(), Ok(Some(_))) {
                        self.wrapper_gone.store(true, Ordering::SeqCst);
                    }
                }
                line = self.out_lines.next_line(), if !self.stdout_done => {
                    match line {
                        Ok(Some(line)) => return ProcessEvent::Stdout(line),
                        _ => self.stdout_done = true,
                    }
                }
                line = self.err_lines.next_line(), if !self.stderr_done => {
                    match line {
                        Ok(Some(line)) => return ProcessEvent::Stderr(line),
                        _ => self.stderr_done = true,
                    }
                }
                status = self.child.wait(), if self.stdout_done && self.stderr_done => {
                    return ProcessEvent::Exit(status);
                }
            }
        }
    }
}
/// Reads the lines `file` gains, as they arrive, until the run is over: its
/// exit file exists, its process group is gone, or `wrapper_gone` is set (the
/// daemon reaped the wrapper shell, which can die before it writes the exit
/// file and then stays a zombie that still counts as alive). What was written
/// before that moment is still delivered.
fn tail_file(
    file: PathBuf,
    exit: PathBuf,
    pgid: i32,
    wrapper_gone: Arc<AtomicBool>,
) -> Box<dyn tokio::io::AsyncRead + Unpin + Send> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut tx, rx) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut offset = 0u64;
        loop {
            let over = exit.exists() || wrapper_gone.load(Ordering::SeqCst) || !group_alive(pgid);
            if let Ok(mut f) = tokio::fs::File::open(&file).await {
                let _ =
                    tokio::io::AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(offset)).await;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match f.read(&mut buf).await {
                        Ok(n) if n > 0 => {
                            offset += n as u64;
                            if tx.write_all(&buf[..n]).await.is_err() {
                                return; // the reader gave up (cancelled run)
                            }
                        }
                        _ => break,
                    }
                }
            }
            if over {
                let _ = tx.shutdown().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    Box::new(rx)
}
