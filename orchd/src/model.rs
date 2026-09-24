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
    /// Flags new tasks start with unless `task.create` overrides them.
    #[serde(default)]
    pub experiments: Variant,
    /// Model id -> price, for harnesses that report no cost.
    #[serde(default = "default_prices")]
    pub prices: std::collections::BTreeMap<String, Price>,
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
            experiments: Variant::default(),
            prices: default_prices(),
        }
    }
}

/// How a retry after a failed attempt starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetryMode {
    /// Continue the failed attempt's session with only the failure.
    #[default]
    Resume,
    /// A new session with the full brief, the earlier attempts' handoffs
    /// and the last failure.
    Fresh,
}

/// The experiment flags a task runs with. `Settings::experiments` is the
/// default; `task.create {variant}` overrides it per task, and the task
/// keeps its own copy, so an A/B pair can run side by side and `orchd ab`
/// groups results by it. A flag that wins becomes the plain behaviour and
/// leaves this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Variant {
    pub retry_mode: RetryMode,
    /// Kill an implement attempt whose harness prints nothing for this
    /// long; 0 = off. The clock pauses during orchd's own Stop-hook verify,
    /// but not during an agent's long Bash call (up to 600s), so values
    /// under ~15 min can kill a busy agent.
    pub stall_timeout_secs: u64,
    /// Route by the tier the planner chose; Jev only when there is none.
    pub planner_tier: bool,
    /// The planner checks the request's claims against the code and pairs
    /// every criterion with how to check it; the reviewer rules on each.
    pub contract: bool,
    /// With review `auto` and a route on the other harness configured, the
    /// reviewer runs there (Claude work reviewed by Codex and back).
    pub review_other_family: bool,
}

#[cfg(test)]
mod price_tests {
    use super::*;

    #[test]
    fn codex_cost_bills_cached_tokens_at_the_cached_rate_once() {
        let p = default_prices()["gpt-5.6-luna"];
        // 359,428 input of which 324,864 cached, 3,869 output (a real run).
        let cost = p.codex_cost(359_428, 324_864, 3_869);
        let expected = (34_564.0 * 0.20 + 324_864.0 * 0.02 + 3_869.0 * 1.20) / 1e6;
        assert!((cost - expected).abs() < 1e-9, "{cost}");
    }
}

impl Task {
    pub fn variant(&self) -> Variant {
        self.variant.clone().unwrap_or_default()
    }
}

/// Per-million-token list prices for a model whose harness reports tokens
/// but no cost (Codex).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
}

impl Price {
    /// Codex's `input_tokens` already includes the cached ones.
    pub fn codex_cost(&self, input: u64, cached: u64, output: u64) -> f64 {
        let fresh = input.saturating_sub(cached) as f64;
        (fresh * self.input + cached as f64 * self.cached_input + output as f64 * self.output)
            / 1_000_000.0
    }
}

// ponytail: list prices as of 2026-09; `settings.prices` overrides them.
fn default_prices() -> std::collections::BTreeMap<String, Price> {
    [
        ("gpt-5.3-codex", 1.75, 0.175, 14.0),
        ("gpt-5.6-luna", 0.20, 0.02, 1.20),
    ]
    .into_iter()
    .map(|(m, input, cached_input, output)| {
        (
            m.to_string(),
            Price {
                input,
                cached_input,
                output,
            },
        )
    })
    .collect()
}

/// Keeps `Instant + timeout` from overflowing.
const MAX_STALL_TIMEOUT_SECS: u64 = 24 * 3600;

impl Variant {
    pub fn check(&self) -> Result<(), String> {
        let s = self.stall_timeout_secs;
        if s > MAX_STALL_TIMEOUT_SECS {
            return Err(format!(
                "stallTimeoutSecs must be at most {MAX_STALL_TIMEOUT_SECS}"
            ));
        }
        Ok(())
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
    /// The branch the task was started from (`base`, or the repo's checked-out
    /// branch). When it moves ahead, the task's work is carried onto it before
    /// verify; `None` (a detached checkout, or a task from before this field)
    /// never rebases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    pub status: TaskStatus,
    pub tier: Tier,
    /// The tier the planner chose, kept even when `variant.planner_tier` is
    /// off so its choice can be compared with Jev's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_tier: Option<Tier>,
    /// `None` only on tasks created before variants existed; `orchd ab`
    /// keeps those out of every arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<Variant>,
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
    /// The harness printed nothing for `variant.stall_timeout_secs`.
    Stall,
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
            FailureKind::Stall => "stall",
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
    /// The agent's note for whoever picks the task up next: what is done,
    /// what was tried, what to do next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
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

/// The address the orchestrator agent sends from and receives at; every
/// other address in a [`Message`] is a task id.
pub const ORCHESTRATOR: &str = "orchestrator";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    Message,
    /// Addressed to the orchestrator: wakes it for a turn.
    Question,
    /// Answers the message named by `reply_to`.
    Reply,
}

/// One agent-to-agent message. The recipient reads it on its next turn --
/// a task's next attempt brief, the orchestrator's next chat turn -- which
/// is when `delivered` flips; nothing interrupts a turn already running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub repo: String,
    pub from: String,
    pub to: String,
    pub kind: MessageKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub ts: i64,
    #[serde(default)]
    pub delivered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<i64>,
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
            base_ref: None,
            status: TaskStatus::Drafting,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: false,
            planned_tier: None,
            variant: Default::default(),
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
            base_ref: None,
            status: TaskStatus::Queued,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: false,
            planned_tier: None,
            variant: Default::default(),
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
            base_ref: None,
            status: TaskStatus::Done,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: true,
            planned_tier: None,
            variant: Default::default(),
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
