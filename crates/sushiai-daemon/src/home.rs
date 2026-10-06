use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The daemon's data directory: socket, lock, state file and holder sockets.
#[derive(Debug, Clone)]
pub struct Home {
    dir: PathBuf,
}

impl Home {
    pub fn new(dir: PathBuf) -> Self {
        Home { dir }
    }

    /// `$SUSHIAI_HOME`, else `~/.sushiai`.
    pub fn from_env() -> Self {
        if let Some(dir) = std::env::var_os("SUSHIAI_HOME") {
            return Home::new(dir.into());
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Home::new(home.join(".sushiai"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("daemon.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.dir.join("daemon.lock")
    }

    pub fn state(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn sessions(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    /// Creates missing directories with mode 0700. An existing directory is never chmod-ed;
    /// it must belong to the current user and not be writable by group or others.
    pub fn ensure(&self) -> Result<()> {
        ensure_dir(&self.dir)?;
        ensure_dir(&self.sessions())
    }
}

fn ensure_dir(dir: &Path) -> Result<()> {
    match fs::metadata(dir) {
        Ok(meta) => {
            // SAFETY: getuid(2) has no preconditions.
            let uid = unsafe { libc::getuid() };
            if meta.uid() != uid || meta.mode() & 0o022 != 0 {
                return Err(Error::UnsafeDir(dir.display().to_string()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
