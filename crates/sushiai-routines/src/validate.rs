//! `routine validate`: everything the daemon would otherwise find out at 09:00.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::argv::{check_access_tool, check_extra_args, harness_argv, RunSpec};
use crate::model::{valid_id, Routine};
use crate::{cron, store::Store};

/// What the caller knows about hosts and projects. The desktop sends the full picture; a
/// daemon that has only its own catalog derives a smaller one ([`Catalog::from_snapshot`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(default)]
    pub hosts: Vec<HostInfo>,
    #[serde(default)]
    pub projects: Vec<ProjectInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub id: String,
    /// Harness name to what the host offers. A harness that is absent is unknown, not denied.
    #[serde(default)]
    pub harnesses: BTreeMap<String, HarnessInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    #[serde(default)]
    pub logged_in: Option<bool>,
    /// The models the host offers; `None` means the list is not known.
    #[serde(default)]
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub folders: Vec<FolderInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FolderInfo {
    pub host: String,
    pub path: String,
}

/// The names one host goes by: `local`, `ssh:name` and `name` are the same host.
pub(crate) fn same_host(a: &str, b: &str) -> bool {
    let bare = |h: &str| h.strip_prefix("ssh:").unwrap_or(h).to_string();
    a == b || bare(a) == bare(b)
}

impl Catalog {
    /// From a daemon `catalog.get` result: the host it names, and every host a folder names.
    pub fn from_snapshot(snapshot: &Value) -> Catalog {
        let mut hosts: Vec<String> = Vec::new();
        let mut projects = Vec::new();
        if let Some(h) = snapshot.get("host").and_then(Value::as_str) {
            hosts.push(h.to_string());
        }
        for p in snapshot
            .get("projects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if p.get("deleted").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let folders: Vec<FolderInfo> = p
                .get("folders")
                .and_then(|f| serde_json::from_value(f.clone()).ok())
                .unwrap_or_default();
            for f in &folders {
                if !hosts.iter().any(|h| same_host(h, &f.host)) {
                    hosts.push(f.host.clone());
                }
            }
            projects.push(ProjectInfo {
                id: p["id"].as_str().unwrap_or_default().to_string(),
                name: p["name"].as_str().unwrap_or_default().to_string(),
                purpose: p["purpose"].as_str().map(String::from),
                remote: p["remote"].as_str().map(String::from),
                folders,
            });
        }
        Catalog {
            hosts: hosts
                .into_iter()
                .map(|id| HostInfo {
                    id,
                    harnesses: BTreeMap::new(),
                })
                .collect(),
            projects,
        }
    }

    pub fn host(&self, id: &str) -> Option<&HostInfo> {
        self.hosts.iter().find(|h| same_host(&h.id, id))
    }

    pub fn project(&self, id: &str) -> Option<&ProjectInfo> {
        self.projects.iter().find(|p| p.id == id || p.name == id)
    }

    /// The folder of `project` on `host`, if the catalog has one.
    pub fn folder(&self, project: &str, host: &str) -> Option<&str> {
        let folders = &self.project(project)?.folders;
        let found = |h: &str| folders.iter().find(|f| same_host(&f.host, h));
        // `local` is this daemon's own host, which a daemon-built catalog lists first.
        found(host)
            .or_else(|| {
                let own = self.hosts.first()?;
                (host == "local").then(|| found(&own.id)).flatten()
            })
            .map(|f| f.path.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ok: bool,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    /// Hash of routine plus brief: the snapshot the daemon runs.
    pub hash: String,
}

impl Report {
    pub fn to_value(&self) -> Value {
        json!(self)
    }

    /// The first error as one line, for a command that fails.
    pub fn first_error(&self) -> String {
        self.errors
            .first()
            .map(|i| format!("{}: {}", i.field, i.message))
            .unwrap_or_default()
    }
}

/// The run modes of `howItRuns.mode`.
pub const MODES: &[&str] = &["ask-every-time", "ask-first", "on-its-own"];
const MAX_ACCESS: usize = 40;

/// Validates `routine` and `brief`. `routine_dir` is the folder the routine lives (or will
/// live) in; `--add-dir` may point only inside its `work/` folder.
pub fn validate(routine: &Routine, brief: &str, catalog: &Catalog, routine_dir: &Path) -> Report {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut err = |field: &str, message: String| {
        errors.push(Issue {
            field: field.into(),
            message,
        })
    };
    if !valid_id(&routine.id) {
        err(
            "id",
            "use lowercase letters, digits and `-`, at most 64".into(),
        );
    }
    if routine.name.trim().is_empty() {
        err("name", "a routine needs a name".into());
    }
    if brief.trim().is_empty() {
        err("brief", "brief.md is empty".into());
    }
    let host = catalog.host(&routine.place.host);
    if host.is_none() {
        err(
            "where.host",
            format!("host {} is not known", routine.place.host),
        );
    }
    if let Some(project) = &routine.place.project {
        match catalog.project(project) {
            None => err("where.project", format!("project {project} does not exist")),
            Some(p) => {
                if !p.folders.is_empty()
                    && catalog.folder(project, &routine.place.host).is_none()
                    && p.remote.is_none()
                {
                    err(
                        "where.project",
                        format!(
                            "project {project} has no folder on {} and no remote to clone",
                            routine.place.host
                        ),
                    );
                }
            }
        }
    }
    if let Err(m) = cron::check(&routine.when.cron) {
        err("when.cron", m);
    }
    if routine.when.tz.parse::<chrono_tz::Tz>().is_err() {
        err(
            "when.tz",
            format!("{} is not an IANA time zone", routine.when.tz),
        );
    }
    let harness = routine.agent.harness.as_str();
    if harness != "claude" && harness != "codex" {
        err("agent.harness", "harness is claude or codex".into());
    } else {
        let probe = RunSpec {
            schema_json: "{}",
            schema_file: Path::new("schema.json"),
            last_message: Path::new("last.json"),
            dry_run: false,
            granted: &[],
        };
        if let Err(m) = harness_argv(&routine.agent, &probe) {
            err("agent.effort", m);
        }
        if let Some(h) = host.and_then(|h| h.harnesses.get(harness)) {
            if let (Some(models), Some(model)) = (&h.models, &routine.agent.model) {
                if !models.iter().any(|m| m == model) {
                    err(
                        "agent.model",
                        format!(
                            "{model} is not offered for {harness} on {}",
                            routine.place.host
                        ),
                    );
                }
            }
            if h.logged_in == Some(false) {
                warnings.push(Issue {
                    field: "agent.harness".into(),
                    message: format!("{harness} is not logged in on {}", routine.place.host),
                });
            }
        }
    }
    if let Some(account) = routine.agent.account.as_deref() {
        // A scheduled run starts from the daemon, which has no way to point a harness at
        // another account's home; saying so now beats running as the wrong login.
        if account != "default" {
            err(
                "agent.account",
                format!(
                    "account {account} is not available to a routine: a scheduled run uses \
                     the host's default login; remove `account` or use \"default\""
                ),
            );
        }
    }
    if let Err(m) = check_extra_args(&routine.agent.extra_args, routine_dir) {
        err("agent.extraArgs", m);
    }
    if !MODES.contains(&routine.how_it_runs.mode.as_str()) {
        err(
            "howItRuns.mode",
            "mode is ask-every-time, ask-first or on-its-own".into(),
        );
    }
    if !(1..=100).contains(&routine.how_it_runs.promote_after) {
        err("howItRuns.promoteAfter", "use 1 to 100".into());
    }
    if routine
        .agent
        .extra_args
        .iter()
        .any(|a| a.starts_with("--allowedTools") || a.starts_with("--allowed-tools"))
    {
        warnings.push(Issue {
            field: "agent.extraArgs".into(),
            message: "--allowedTools is ignored: a run may use only the tools in `access` that \
                      the owner granted"
                .into(),
        });
    }
    if routine.access.len() > MAX_ACCESS {
        err("access", format!("at most {MAX_ACCESS} entries"));
    }
    let mut seen: Vec<&str> = Vec::new();
    for (i, a) in routine.access.iter().enumerate() {
        let field = format!("access[{i}]");
        if let Err(m) = check_access_tool(&a.tool) {
            err(&format!("{field}.tool"), m);
        } else if harness == "codex" && a.tool.starts_with("mcp__") {
            err(
                &format!("{field}.tool"),
                "Codex runs cannot be granted MCP tools yet; use Claude".into(),
            );
        }
        if seen.contains(&a.tool.as_str()) {
            err(
                &format!("{field}.tool"),
                format!("{} is listed twice", a.tool),
            );
        }
        seen.push(&a.tool);
        if a.label.trim().is_empty() || a.label.chars().count() > 120 {
            err(
                &format!("{field}.label"),
                "a label of 1 to 120 characters".into(),
            );
        }
        if a.detail.as_deref().is_some_and(|d| d.chars().count() > 500) {
            err(&format!("{field}.detail"), "at most 500 characters".into());
        }
        if a.kind != "read" && a.kind != "write" {
            err(&format!("{field}.kind"), "kind is read or write".into());
        }
    }
    if routine.limits.timeout_min == 0 || routine.limits.timeout_min > 24 * 60 {
        err("limits.timeoutMin", "use 1 to 1440 minutes".into());
    }
    Report {
        ok: errors.is_empty(),
        errors,
        warnings,
        hash: Store::hash(routine, brief),
    }
}

#[cfg(test)]
mod local_alias_tests {
    use super::*;

    #[test]
    fn local_names_the_daemons_own_host() {
        let snapshot = serde_json::json!({
            "host": "build-mac",
            "projects": [{"id": "my-app", "name": "my-app",
                          "folders": [{"host": "build-mac", "path": "/work/my-app"}]}]
        });
        let catalog = Catalog::from_snapshot(&snapshot);
        assert_eq!(catalog.folder("my-app", "build-mac"), Some("/work/my-app"));
        assert_eq!(catalog.folder("my-app", "local"), Some("/work/my-app"));
        assert_eq!(catalog.folder("my-app", "other-host"), None);
    }
}
