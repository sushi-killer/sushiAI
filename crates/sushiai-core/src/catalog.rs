//! Pure catalog logic: last-writer merge, replica filter, cwd matching and the state-file
//! section. No IO, no clock. Session bindings are opaque to the daemon and are not checked
//! here.
//!
//! Decisions (v1):
//! - The desktop is master. A record wins on higher `rev`, or equal `rev` and
//!   higher `updatedAt`; equal on both is ignored (idempotent re-apply).
//! - Deletions are tombstones (`deleted: true`) and are never pruned in v1.
//! - A full sync is authoritative: every record in the payload replaces the stored one
//!   whatever its `rev`, and live records absent from it become tombstones. Tombstones made
//!   by a sync (absent records, cascaded groups) keep `rev` and `updatedAt`: there is no
//!   clock here, and the next full sync decides again.
//! - A daemon keeps a project only while it has a folder on this host.
//!   A stored project that loses its folder here becomes a tombstone.
//! - Deleting a project tombstones its groups; a group comes back with the next full groups
//!   sync that lists it, once its project is live again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sushiai_protocol::catalog::{Group, GroupsSync, Project, ProjectsSync, SyncResult};

pub const CATALOG_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog version {0:?} is not supported (expected {CATALOG_VERSION})")]
    UnsupportedVersion(Option<u64>),
    #[error("catalog is not valid: {0}")]
    Invalid(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub projects: BTreeMap<String, Project>,
    pub groups: BTreeMap<String, Group>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogFile {
    catalog_version: u32,
    projects: Vec<Project>,
    groups: Vec<Group>,
}

fn newer(rev: u64, updated_at: u64, old_rev: u64, old_updated_at: u64) -> bool {
    (rev, updated_at) > (old_rev, old_updated_at)
}

fn on_host(project: &Project, host: &str) -> bool {
    project.folders.iter().any(|f| f.host == host)
}

fn components(path: &str) -> Vec<&str> {
    path.split('/').filter(|c| !c.is_empty()).collect()
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_rev(&self) -> u64 {
        let p = self.projects.values().map(|p| p.rev);
        let g = self.groups.values().map(|g| g.rev);
        p.chain(g).max().unwrap_or(0)
    }

    fn result(&self, applied: u32, ignored: u32) -> SyncResult {
        SyncResult {
            applied,
            ignored,
            rev: self.max_rev(),
        }
    }

    pub fn apply_projects(&mut self, sync: &ProjectsSync, this_host: &str) -> SyncResult {
        if sync.host != this_host {
            let ignored = sync.projects.len() as u32;
            return self.result(0, ignored);
        }
        let (mut applied, mut ignored) = (0, 0);
        for incoming in &sync.projects {
            let old = self.projects.get(&incoming.id);
            let mut record = incoming.clone();
            if !record.deleted && !on_host(&record, this_host) {
                record.deleted = true;
            }
            let wins = match old {
                // Nothing to tombstone and not a replica here: not ours.
                None => !incoming.deleted && on_host(incoming, this_host),
                // A full sync is authoritative whatever the rev.
                Some(o) if sync.full => *o != record,
                Some(o) => newer(incoming.rev, incoming.updated_at, o.rev, o.updated_at),
            };
            if !wins {
                ignored += 1;
                continue;
            }
            self.projects.insert(record.id.clone(), record);
            applied += 1;
        }
        if sync.full {
            let present: Vec<&str> = sync.projects.iter().map(|p| p.id.as_str()).collect();
            for p in self.projects.values_mut() {
                if !p.deleted && !present.contains(&p.id.as_str()) {
                    p.deleted = true;
                    applied += 1;
                }
            }
        }
        applied += self.cascade_groups();
        self.result(applied, ignored)
    }

    pub fn apply_groups(&mut self, sync: &GroupsSync) -> SyncResult {
        let (mut applied, mut ignored) = (0, 0);
        for incoming in &sync.groups {
            let project = self.projects.get(&incoming.project_id);
            let kept = project.is_some_and(|p| !p.deleted || incoming.deleted);
            let wins = kept
                && match self.groups.get(&incoming.id) {
                    Some(o) if sync.full => o != incoming,
                    Some(o) => newer(incoming.rev, incoming.updated_at, o.rev, o.updated_at),
                    None => !incoming.deleted,
                };
            if !wins {
                ignored += 1;
                continue;
            }
            self.groups.insert(incoming.id.clone(), incoming.clone());
            applied += 1;
        }
        if sync.full {
            let present: Vec<&str> = sync.groups.iter().map(|g| g.id.as_str()).collect();
            for g in self.groups.values_mut() {
                if !g.deleted && !present.contains(&g.id.as_str()) {
                    g.deleted = true;
                    applied += 1;
                }
            }
        }
        self.result(applied, ignored)
    }

    /// Tombstones live groups whose project is deleted or gone.
    fn cascade_groups(&mut self) -> u32 {
        let mut n = 0;
        for g in self.groups.values_mut() {
            let alive = self.projects.get(&g.project_id).is_some_and(|p| !p.deleted);
            if !g.deleted && !alive {
                g.deleted = true;
                n += 1;
            }
        }
        n
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, CatalogError> {
        let file = CatalogFile {
            catalog_version: CATALOG_VERSION,
            projects: self.projects.values().cloned().collect(),
            groups: self.groups.values().cloned().collect(),
        };
        Ok(serde_json::to_vec_pretty(&file)?)
    }

    /// Checks the catalog version before reading anything else.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CatalogError> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let version = value.get("catalogVersion").and_then(|v| v.as_u64());
        if version != Some(u64::from(CATALOG_VERSION)) {
            return Err(CatalogError::UnsupportedVersion(version));
        }
        let file: CatalogFile = serde_json::from_value(value)?;
        Ok(Catalog {
            projects: file
                .projects
                .into_iter()
                .map(|p| (p.id.clone(), p))
                .collect(),
            groups: file.groups.into_iter().map(|g| (g.id.clone(), g)).collect(),
        })
    }
}

/// Longest folder match on path components; ties go to the smaller id.
pub fn project_for_cwd(catalog: &Catalog, host: &str, cwd: &str) -> Option<String> {
    let cwd = components(cwd);
    let mut best: Option<(usize, &str)> = None;
    for p in catalog.projects.values().filter(|p| !p.deleted) {
        for f in p.folders.iter().filter(|f| f.host == host) {
            let folder = components(&f.path);
            if cwd.starts_with(&folder) && best.is_none_or(|(n, _)| folder.len() > n) {
                best = Some((folder.len(), p.id.as_str()));
            }
        }
    }
    best.map(|(_, id)| id.to_string())
}
