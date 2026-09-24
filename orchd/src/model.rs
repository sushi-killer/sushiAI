//! Wire types for the orchestrator protocol and stored state. Field names
//! are camelCase to match `artifacts/tasks/orchestrator-mvp.md` exactly,
//! since the Electron/UI lane (Lane B) serializes/deserializes the same
//! shapes independently.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub label: String,
    pub harness: Harness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Claude only: a named model-profile (env/apiKeyHelper) merged into the
    /// run's `settings.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Mechanical,
    Standard,
    Hard,
}

impl Tier {
    /// mechanical -> standard -> hard -> hard (caps out).
    pub fn up(self) -> Tier {
        match self {
            Tier::Mechanical => Tier::Standard,
            Tier::Standard => Tier::Hard,
            Tier::Hard => Tier::Hard,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Mechanical => "mechanical",
            Tier::Standard => "standard",
            Tier::Hard => "hard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    Native,
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassifierBackend {
    None,
    Openrouter,
    Typesafe,
    Openai,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierSettings {
    pub backend: ClassifierBackend,
    pub model: String,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub routes: Vec<Route>,
    pub tiers: HashMap<Tier, String>,
    /// Route id; "" = off, "auto" = first configured route whose harness
    /// differs from the implement attempt's.
    pub review: String,
    pub classifier: ClassifierSettings,
    pub sandbox: SandboxMode,
    pub allowed_domains: Vec<String>,
    /// Codex's sandbox has no per-domain filter, just a network on/off
    /// switch (spec assumption); this is that switch. Default `false` --
    /// `allowedDomains` only ever governs Claude's allowlist.
    #[serde(default)]
    pub codex_network: bool,
    pub protected_paths: Vec<String>,
    pub max_attempts: u32,
    pub parallel: u32,
    /// Route id used for the drafting/plan stage of a `{repo, request}`
    /// `task.create`; `""` turns planning off (that create form is then
    /// rejected -- there is nothing to run it with).
    #[serde(default = "default_planner")]
    pub planner: String,
    /// Route the orchestrator agent runs on: its chat with the owner and,
    /// with `auto_answer`, its triage of stuck questions. `""` (or an id that
    /// matches no route) means the standard tier's route.
    #[serde(default)]
    pub orchestrator: String,
    /// Whether the orchestrator answers a stuck agent's question itself
    /// before it reaches the owner. Off by default: the owner opts in to
    /// answers given on their behalf.
    #[serde(default)]
    pub auto_answer: bool,
}

fn default_planner() -> String {
    "claude-opus".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        let mut tiers = HashMap::new();
        tiers.insert(Tier::Mechanical, "codex".to_string());
        tiers.insert(Tier::Standard, "claude-sonnet".to_string());
        tiers.insert(Tier::Hard, "claude-opus".to_string());
        Settings {
            routes: vec![
                Route {
                    id: "claude-sonnet".to_string(),
                    label: "Claude Sonnet".to_string(),
                    harness: Harness::Claude,
                    model: Some("sonnet".to_string()),
                    effort: None,
                    profile_id: None,
                },
                Route {
                    id: "claude-opus".to_string(),
                    label: "Claude Opus".to_string(),
                    harness: Harness::Claude,
                    model: Some("opus".to_string()),
                    effort: Some("high".to_string()),
                    profile_id: None,
                },
                Route {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    harness: Harness::Codex,
                    model: None,
                    effort: None,
                    profile_id: None,
                },
            ],
            tiers,
            review: "auto".to_string(),
            classifier: ClassifierSettings {
                backend: ClassifierBackend::Openrouter,
                model: "typesafe/jev-1.13".to_string(),
                provider_id: String::new(),
            },
            sandbox: SandboxMode::Native,
            // Open network by default: the sandbox's job is keeping writes
            // inside the worktree, not blocking package registries.
            allowed_domains: vec!["*".to_string()],
            codex_network: false,
            protected_paths: vec![],
            max_attempts: 4,
            parallel: 2,
            planner: default_planner(),
            orchestrator: String::new(),
            auto_answer: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// A `{repo, request}` task.create is drafting its own title/goal/
    /// criteria/verify (and asking any clarifying questions) before it ever
    /// reaches `queued`.
    Drafting,
    Queued,
    Running,
    Waiting,
    Done,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub criteria: Vec<String>,
    pub verify: Vec<String>,
    /// The owner's raw one-sentence ask, set only by the `{repo, request}`
    /// form of `task.create`; the drafting stage fills `title`/`goal`/
    /// `criteria`/`verify` from it and leaves this as the original text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    pub repo: String,
    pub worktree: String,
    pub branch: String,
    pub base_sha: String,
    pub status: TaskStatus,
    pub tier: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub cost_usd: f64,
    /// Hides the task from the default `task.list` without deleting it.
    /// `#[serde(default)]` so a `task.json` written before this field
    /// existed still loads, with `archived: false`.
    #[serde(default)]
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// Drafting: a fresh read-only session that fills in title/goal/
    /// criteria/verify from the owner's one-sentence request.
    Plan,
    Implement,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptStatus {
    Running,
    Passed,
    Failed,
    Interrupted,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyOutcome {
    pub command: String,
    pub code: Option<i32>,
    pub tail: String,
    pub ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResult {
    pub verdict: Verdict,
    #[serde(default)]
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    NoDeliverable,
    Verify,
    Review,
    Protected,
    Blocked,
    Error,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::NoDeliverable => "no_deliverable",
            FailureKind::Verify => "verify",
            FailureKind::Review => "review",
            FailureKind::Protected => "protected",
            FailureKind::Blocked => "blocked",
            FailureKind::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub kind: FailureKind,
    pub detail: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cached: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub n: u32,
    pub stage: Stage,
    pub route_id: String,
    pub harness: Harness,
    pub model: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The spawned child's process group id (== its pid, since it calls
    /// `setsid()`), persisted as soon as it's known so a killed daemon can
    /// `killpg` it back on recovery instead of leaving it orphaned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<i32>,
    pub resumed: bool,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub status: AttemptStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub verify: Vec<VerifyOutcome>,
    #[serde(default)]
    pub gate_blocks: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_round_trips_through_json() {
        let s = Settings::default();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["review"], "auto");
        assert_eq!(v["tiers"]["mechanical"], "codex");
        assert_eq!(v["tiers"]["standard"], "claude-sonnet");
        assert_eq!(v["tiers"]["hard"], "claude-opus");
        assert_eq!(v["maxAttempts"], 4);
        assert_eq!(v["parallel"], 2);
        assert_eq!(v["classifier"]["backend"], "openrouter");
        assert_eq!(v["classifier"]["model"], "typesafe/jev-1.13");
        assert_eq!(v["planner"], "claude-opus");
        assert_eq!(v["orchestrator"], "");
        let back: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(back.max_attempts, 4);
        assert_eq!(back.tiers.get(&Tier::Hard).unwrap(), "claude-opus");
    }

    #[test]
    fn settings_default_without_a_planner_field_defaults_to_claude_opus() {
        // An older settings.json on disk (written before this field existed)
        // must still load, with planning on by default and on the hard
        // tier's route - the strongest model plans best.
        let old_json = serde_json::json!({
            "routes": [], "tiers": {}, "review": "", "classifier": {"backend": "none", "model": "", "providerId": ""},
            "sandbox": "host", "allowedDomains": [], "protectedPaths": [], "maxAttempts": 4, "parallel": 2,
        });
        let s: Settings = serde_json::from_value(old_json).unwrap();
        assert_eq!(s.planner, "claude-opus");
        assert_eq!(s.orchestrator, "");
        assert!(!s.auto_answer);
    }

    #[test]
    fn task_request_field_is_camel_case_and_omitted_when_absent() {
        let task = Task {
            id: "t1".into(),
            title: "Fix the thing".into(),
            goal: String::new(),
            criteria: vec![],
            verify: vec![],
            request: Some("fix the thing that's broken".into()),
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            branch: "task/x".into(),
            base_sha: "abc".into(),
            status: TaskStatus::Drafting,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: false,
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["request"], "fix the thing that's broken");
        assert_eq!(v["status"], "drafting");

        let mut without_request = task;
        without_request.request = None;
        let v2 = serde_json::to_value(&without_request).unwrap();
        assert!(v2.get("request").is_none());
    }

    #[test]
    fn tier_up_caps_at_hard() {
        assert_eq!(Tier::Mechanical.up(), Tier::Standard);
        assert_eq!(Tier::Standard.up(), Tier::Hard);
        assert_eq!(Tier::Hard.up(), Tier::Hard);
    }

    #[test]
    fn task_field_names_are_camel_case() {
        let task = Task {
            id: "t1".into(),
            title: "Title".into(),
            goal: "Goal".into(),
            criteria: vec![],
            verify: vec![],
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            branch: "task/x".into(),
            base_sha: "abc".into(),
            status: TaskStatus::Queued,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: false,
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["baseSha"], "abc");
        assert_eq!(v["createdAt"], 1);
        assert!(v.get("base_sha").is_none());
    }

    #[test]
    fn task_archived_field_is_camel_case_and_defaults_to_false_when_absent() {
        let mut task = Task {
            id: "t1".into(),
            title: "Title".into(),
            goal: "Goal".into(),
            criteria: vec![],
            verify: vec![],
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            branch: "task/x".into(),
            base_sha: "abc".into(),
            status: TaskStatus::Done,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: true,
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["archived"], true);

        // A pre-existing task.json written before this field existed must
        // still load, defaulting to `archived: false`.
        let mut without_archived = v.clone();
        without_archived.as_object_mut().unwrap().remove("archived");
        let loaded: Task = serde_json::from_value(without_archived).unwrap();
        assert!(!loaded.archived);

        task.archived = false;
        let v2 = serde_json::to_value(&task).unwrap();
        assert_eq!(v2["archived"], false);
    }
}
