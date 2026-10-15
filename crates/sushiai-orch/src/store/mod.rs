//! Atomic JSON persistence and run artifact locations.

mod audits;
pub mod costs;
pub mod files;
mod proposals;
mod settings;
mod task;

use std::io;
use std::path::{Path, PathBuf};

pub struct Store {
    data_dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: impl Into<PathBuf>) -> io::Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(data_dir.join("tasks"))?;
        Ok(Store { data_dir })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

#[cfg(test)]
mod tests;

pub(crate) mod log;

pub(crate) mod permissions;
