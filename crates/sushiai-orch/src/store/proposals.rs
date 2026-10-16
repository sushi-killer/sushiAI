use super::files::{read_json, write_json_atomic};
use super::Store;
use crate::task::evolution::Proposal;
use std::fs;
use std::io;
use std::path::PathBuf;

impl Store {
    pub fn proposals_dir(&self) -> PathBuf {
        self.data_dir.join("evolution").join("proposals")
    }

    /// The directory of a proposer run: its brief and events.
    pub fn proposal_run_dir(&self, id: &str) -> PathBuf {
        self.proposals_dir().join(id)
    }

    pub fn proposal_path(&self, id: &str) -> PathBuf {
        self.proposals_dir().join(format!("{id}.json"))
    }

    pub fn save_proposal(&self, proposal: &Proposal) -> io::Result<()> {
        write_json_atomic(&self.proposal_path(&proposal.id), proposal)
    }

    pub fn load_proposal(&self, id: &str) -> io::Result<Option<Proposal>> {
        read_json(&self.proposal_path(id))
    }

    /// Every stored proposal, newest first; corrupt entries are skipped.
    pub fn list_proposals(&self) -> io::Result<Vec<Proposal>> {
        let dir = self.proposals_dir();
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(Some(proposal)) = read_json::<Proposal>(&path) {
                out.push(proposal);
            }
        }
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }
}
