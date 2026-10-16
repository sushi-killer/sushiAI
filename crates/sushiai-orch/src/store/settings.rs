use super::files::{read_json, write_json_atomic};
use super::Store;
use crate::policy::settings::Settings;
use crate::task::notes::RepoNote;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

impl Store {
    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn load_settings(&self) -> io::Result<Settings> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text = fs::read_to_string(&path)?;
        let settings: Settings = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json_atomic(&self.settings_path(), settings)
    }

    pub fn repo_notes_path(&self) -> PathBuf {
        self.data_dir.join("repo-notes.json")
    }

    /// Every repo's notes, keyed by repo root.
    pub fn load_repo_notes(&self) -> io::Result<BTreeMap<String, Vec<RepoNote>>> {
        Ok(read_json(&self.repo_notes_path())?.unwrap_or_default())
    }

    pub fn save_repo_notes(&self, notes: &BTreeMap<String, Vec<RepoNote>>) -> io::Result<()> {
        write_json_atomic(&self.repo_notes_path(), notes)
    }
}
