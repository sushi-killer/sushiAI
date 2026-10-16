use crate::harness::cancel::CancelToken;
use crate::harness::guard::{Guard, Trip};
use crate::harness::run_files::RunFiles;
use crate::harness::{self, OutputSchema, RunRequest};
use crate::policy::routing::Harness;
use std::path::Path;

pub(crate) struct NativeCommand {
    binary: String,
    argv: Vec<String>,
}

pub(crate) struct PreparedCommand {
    pub(crate) command: tokio::process::Command,
    pub(super) argv: Vec<String>,
    pub(super) codex_version: Option<String>,
    pub(super) binary: String,
}

impl NativeCommand {
    pub(crate) fn new(req: &RunRequest<'_>, schema: Option<&OutputSchema<'_>>) -> Self {
        let mut argv = harness::build_argv_with_schema(req, schema);
        let mut binary = crate::harness::environment::resolve_binary(req.harness);
        #[cfg(unix)]
        if req.harness == Harness::Claude {
            if let Some(settings_path) = req.settings_path {
                let token_path = settings_path.with_file_name("oauth-token");
                if token_path.is_file() {
                    let claude = binary;
                    let args = argv;
                    binary = "/bin/sh".into();
                    argv = vec!["-c".into(),
                        "exec 3<\"$1\"; rm -f \"$1\"; export CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3; shift; exec \"$@\"".into(),
                        "sushiai".into(), token_path.to_string_lossy().into_owned(), claude];
                    argv.extend(args);
                }
            }
        }
        Self { binary, argv }
    }

    pub(crate) async fn version(
        &self,
        harness: Harness,
        cancel: &CancelToken,
        guard: &mut Guard,
    ) -> Result<Option<String>, Trip> {
        match harness {
            Harness::Codex => super::version::codex_version(&self.binary, cancel, guard).await,
            Harness::Claude => Ok(None),
        }
    }

    pub(crate) fn prepare(
        self,
        req: &RunRequest<'_>,
        files: Option<&RunFiles>,
        brief: &str,
        codex_version: Option<String>,
    ) -> PreparedCommand {
        let run_argv = self.argv.clone();
        let (mut binary, mut argv) = wrap_task_harness(self.binary, self.argv, req);
        if let Some(files) = files {
            let wrapped = std::mem::take(&mut argv);
            argv = vec![
                "-c".into(),
                "\"$@\"; c=$?; echo $c > \"$0.tmp\" && mv \"$0.tmp\" \"$0\"; exit $c".into(),
                files.exit.to_string_lossy().into_owned(),
                binary,
            ];
            argv.extend(wrapped);
            binary = "/bin/sh".into();
            use std::io::Write;
            let _ = create_private(&files.stdin).and_then(|mut f| f.write_all(brief.as_bytes()));
            let _ = std::fs::remove_file(&files.exit);
            let _ = std::fs::write(
                &files.meta,
                serde_json::json!({"argv":&run_argv,"codexVersion":codex_version}).to_string(),
            );
        }
        let mut command = tokio::process::Command::new(&binary);
        command.args(&argv);
        PreparedCommand {
            command,
            argv: run_argv,
            codex_version,
            binary,
        }
    }
}
pub(super) fn wrap_task_harness(
    bin: String,
    argv: Vec<String>,
    req: &harness::RunRequest<'_>,
) -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    if req.harness == Harness::Codex && harness::task_run_uses_outer_sandbox(req) {
        if let Some(policy) = req.task_sandbox {
            let mut wrapped = vec!["-p".into(), crate::sandbox::to_sbpl(policy), bin];
            wrapped.extend(argv);
            return ("sandbox-exec".into(), wrapped);
        }
    }
    let _ = req;
    (bin, argv)
}
/// Creates `path` empty, readable by the owner only.
pub(super) fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}
