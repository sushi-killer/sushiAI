//! One compiler cache for orchd's worktrees; build outputs stay in each worktree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

pub const SERVER_PORT: u16 = 4227;

#[derive(Clone, Debug)]
pub struct CompilerCache {
    executable: PathBuf,
    path: String,
    pub dir: PathBuf,
    pub port: u16,
}

impl CompilerCache {
    /// Explicit inputs keep tests away from the owner's cache and server.
    pub fn discover(
        path: &str,
        enabled: bool,
        owner_set_wrapper: bool,
        dir: PathBuf,
        port: u16,
    ) -> Option<Self> {
        if !enabled || owner_set_wrapper {
            return None;
        }
        let executable = std::env::split_paths(path)
            .map(|d| d.join("sccache"))
            .find(|p| is_executable(p))?
            .canonicalize()
            .ok()?;
        Some(Self {
            executable,
            path: path.to_string(),
            dir,
            port,
        })
    }

    pub fn for_worktree(worktree: &Path, path: &str) -> Option<Self> {
        // A unit test never joins the owner's server (port 4227) or fills the owner's
        // cache directory: there, only `discover` with its own directory and port builds one.
        if cfg!(test) {
            return None;
        }
        let home = std::env::var_os("HOME")?;
        Self::discover(
            path,
            crate::worktree_env::compiler_cache_enabled(worktree).unwrap_or(false),
            std::env::var_os("RUSTC_WRAPPER").is_some(),
            PathBuf::from(home).join(".cache/sushiai/sccache"),
            SERVER_PORT,
        )
    }

    fn server_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.executable);
        cmd.env("PATH", &self.path)
            .env("RUSTC_WRAPPER", "sccache")
            .env("SCCACHE_DIR", &self.dir)
            .env("SCCACHE_SERVER_PORT", self.port.to_string())
            .env_remove("SCCACHE_SERVER_UDS")
            // The server must outlive the worktree that first needed it.
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    /// Invoked by the daemon before spawning a sandbox or a check's setsid.
    /// An existing server is kept; an idle server can be started again.
    pub fn start_server(&self) {
        let _ = self
            .server_command()
            .arg("--start-server")
            .stdout(Stdio::null())
            .status();
    }

    pub fn apply(&self, command: &mut tokio::process::Command) {
        self.start_server();
        command
            .env("RUSTC_WRAPPER", "sccache")
            .env("SCCACHE_DIR", &self.dir)
            .env("SCCACHE_SERVER_PORT", self.port.to_string())
            .env_remove("SCCACHE_SERVER_UDS");
    }

    pub async fn snapshot(&self) -> Option<Stats> {
        let mut command = tokio::process::Command::from(self.server_command());
        command
            .args(["--show-stats", "--stats-format=json"])
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(5), command.output())
            .await
            .ok()?
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Stats::from_json(&output.stdout)
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub requests: u64,
    pub hits: u64,
}

impl Stats {
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        #[derive(Deserialize)]
        struct Hits {
            counts: HashMap<String, u64>,
        }
        #[derive(Deserialize)]
        struct Counters {
            compile_requests: u64,
            cache_hits: Hits,
        }
        #[derive(Deserialize)]
        struct Info {
            stats: Counters,
        }
        let info: Info = serde_json::from_slice(bytes).ok()?;
        Some(Self {
            requests: info.stats.compile_requests,
            hits: info.stats.cache_hits.counts.values().sum(),
        })
    }

    pub fn delta(self, before: Self) -> Option<Self> {
        Some(Self {
            requests: self.requests.checked_sub(before.requests)?,
            hits: self.hits.checked_sub(before.hits)?,
        })
    }

    pub fn decision(self, gate: &str) -> Option<String> {
        if self.requests == 0 {
            return None;
        }
        let percent = (u128::from(self.hits) * 100 + u128::from(self.requests) / 2)
            / u128::from(self.requests);
        Some(format!(
            "Orchestrator: compiler cache {}/{} hits ({percent}%) in {gate}",
            self.hits, self.requests
        ))
    }
}

/// A gate's snapshots use the daemon's server, outside the command sandbox.
pub struct CacheRun {
    cache: Option<CompilerCache>,
    before: Option<Stats>,
}

impl CacheRun {
    pub async fn begin(cache: Option<CompilerCache>) -> Self {
        let before = match &cache {
            Some(cache) => {
                cache.start_server();
                cache.snapshot().await
            }
            None => None,
        };
        Self { cache, before }
    }

    pub(super) async fn for_commands(worktree: &Path, commands: &[String]) -> Self {
        let cache = if commands.is_empty() {
            None
        } else {
            CompilerCache::for_worktree(worktree, &super::augmented_path())
        };
        Self::begin(cache).await
    }

    pub async fn finish(self, gate: &str) -> Option<String> {
        let before = self.before?;
        self.cache?.snapshot().await?.delta(before)?.decision(gate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rate_uses_the_delta_and_silences_empty_or_reset_counters() {
        let before = Stats {
            requests: 100,
            hits: 70,
        };
        let after = Stats {
            requests: 152,
            hits: 111,
        };
        assert_eq!(
            after
                .delta(before)
                .unwrap()
                .decision("final checks")
                .as_deref(),
            Some("Orchestrator: compiler cache 41/52 hits (79%) in final checks")
        );
        assert_eq!(before.delta(before).unwrap().decision("verify"), None);
        assert_eq!(before.delta(after), None);
        assert_eq!(
            Stats {
                requests: 3,
                hits: 2
            }
            .decision("parent gate")
            .unwrap(),
            "Orchestrator: compiler cache 2/3 hits (67%) in parent gate"
        );
    }

    #[test]
    fn json_counts_languages_once_and_ignores_advanced_duplicates() {
        let stats = Stats::from_json(br#"{"stats":{"compile_requests":52,"cache_hits":{"counts":{"Rust":40,"C/C++":1},"adv_counts":{"rustc":40,"clang":1}}}}"#).unwrap();
        assert_eq!(
            stats,
            Stats {
                requests: 52,
                hits: 41
            }
        );
        assert_eq!(Stats::from_json(b"not json"), None);
        assert_eq!(Stats::from_json(b"{}"), None);
    }
}
