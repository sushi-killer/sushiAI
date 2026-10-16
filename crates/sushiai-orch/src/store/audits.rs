use super::files::{read_json, write_json_atomic};
use super::Store;
use crate::task::audit::{Audit, AuditReport};
use std::fs;
use std::io;
use std::path::PathBuf;

impl Store {
    pub fn audit_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("audits").join(id)
    }

    pub fn save_audit(&self, audit: &Audit) -> io::Result<()> {
        write_json_atomic(&self.audit_dir(&audit.id).join("audit.json"), audit)
    }

    pub fn load_audit(&self, id: &str) -> io::Result<Option<Audit>> {
        read_json(&self.audit_dir(id).join("audit.json"))
    }

    pub fn save_audit_report(&self, id: &str, report: &AuditReport) -> io::Result<()> {
        write_json_atomic(&self.audit_dir(id).join("report.json"), report)
    }

    pub fn load_audit_report(&self, id: &str) -> io::Result<Option<AuditReport>> {
        read_json(&self.audit_dir(id).join("report.json"))
    }

    /// Every audit of `repo`, newest first; corrupt entries are skipped.
    pub fn list_audits(&self, repo: &str) -> io::Result<Vec<Audit>> {
        let dir = self.data_dir.join("audits");
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Ok(Some(audit)) = read_json::<Audit>(&entry.path().join("audit.json")) {
                if audit.repo == repo {
                    out.push(audit);
                }
            }
        }
        out.sort_by_key(|audit| std::cmp::Reverse(audit.started_at));
        Ok(out)
    }
}
