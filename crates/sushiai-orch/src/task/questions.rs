pub(crate) mod operations;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
    /// What the question is about; the answer policy picks its rule by it.
    #[serde(default)]
    pub kind: QuestionKind,
    /// The stage that asked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<AskedBy>,
    /// When it was asked (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<i64>,
    /// An overseer turn was started (or queued) for this question and has not
    /// ended: the owner is not asked yet. Cleared when the turn ends with the
    /// question still open, or when no turn can run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held_for_overseer: bool,
    /// When the hold began (ms). A hold older than [`HOLD_TTL_MS`] no longer
    /// counts, so a hung overseer turn cannot hide a question for good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_at: Option<i64>,
    /// orchd handed this question to the owner: neither the overseer, triage,
    /// the answer policy nor a precedent will settle it. The app notifies,
    /// badges and counts a waiting task only then. Once set it stays set
    /// until the question is answered or replaced; a new question starts
    /// without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub for_owner: bool,
    /// The check commands a base-check question is about, in listing order.
    /// Empty for every other question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_signature: Option<String>,
    /// What the asking agent says the answer commits the owner to. A tag only
    /// adds caution: a tagged question is the owner's, whatever its words say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk: Vec<Risk>,
    /// The option (an index into `options`) the asking agent would take when
    /// nobody answers in time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_option: Option<usize>,
    /// The criterion an impossible question is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<usize>,
    /// The agent's evidence for an impossible question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// What each of `commands` printed on the base, in the same order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base_tails: Vec<String>,
}

/// A kind of commitment the asking agent can tag a question with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Push,
    Money,
    Destructive,
    Security,
    Deploy,
    Scope,
    #[serde(rename = "public-api")]
    PublicApi,
}

/// How long a question stays held for the overseer.
pub const HOLD_TTL_MS: i64 = 10 * 60 * 1000;

impl Question {
    /// Whether the overseer still holds this question at `now`.
    pub fn held_now(&self, now: i64) -> bool {
        self.held_for_overseer && self.held_at.is_some_and(|at| now - at < HOLD_TTL_MS)
    }

    pub fn release(&mut self) {
        self.held_for_overseer = false;
        self.held_at = None;
    }
}

impl Question {
    /// A question asked now by `asked_by`; every asking site goes through it.
    pub fn new(
        text: impl Into<String>,
        options: Vec<String>,
        kind: QuestionKind,
        asked_by: AskedBy,
        now: i64,
    ) -> Self {
        Question {
            text: text.into(),
            options,
            kind,
            asked_by: Some(asked_by),
            asked_at: Some(now),
            held_for_overseer: false,
            held_at: None,
            for_owner: false,
            commands: Vec::new(),
            failure_signature: None,
            risk: Vec::new(),
            default_option: None,
            criterion: None,
            evidence: None,
            base_tails: Vec::new(),
        }
    }
}

/// The stage that asked a question. `brief`, `advisor` and `land` are
/// reserved for the UI: nothing asks from them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AskedBy {
    Brief,
    Plan,
    Implement,
    Verify,
    Review,
    Advisor,
    Land,
}

/// Who answered a question kept in the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnsweredBy {
    Owner,
    Policy,
    Judge,
    Orchestrator,
    Lead,
    Overseer,
}

/// An answered question, kept in `Task.questionHistory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnsweredQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub kind: QuestionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<AskedBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<i64>,
    pub answer: String,
    pub answered_at: i64,
    pub answered_by: AnsweredBy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_signature: Option<String>,
    /// The check commands a base-check question was about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

/// Why orchd asks a question, set where it is asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    AttemptsFailing,
    ReviewNoVerdict,
    DependencyEnded,
    Impossible,
    PreexistingFailure,
    Budget,
    ProtectedPath,
    /// A tool call or path write the run is not allowed to make; answered by
    /// the owner only.
    Permission,
    /// A review finding the implementer declined and the review upheld.
    ReviewDispute,
    PlanQuestion,
    /// Today's spend reached `dailyBudgetUsd`; never answered by policy.
    DailyBudget,
    /// No agent CLI (claude or codex) is installed; never answered by policy.
    HarnessMissing,
    /// A post-carry PR check still fails after its own repair step.
    PrCheck,
    #[default]
    AgentQuestion,
}

/// A choice the planner made itself instead of asking the owner
/// (`variant.batch_questions`). The owner can overturn it later with
/// `task.overturn`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assumption {
    pub question: String,
    /// The planner's recommended option.
    pub answer: String,
    pub evidence: String,
    /// `planner`, or for the answer policy `policy` (a rule) / `judge`.
    pub by: String,
    /// The kind of question the answer policy answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<QuestionKind>,
    /// The attempt it was asked in (answer policy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// The check commands a base-check answer covered (answer policy only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    pub overturned: bool,
    /// What the owner answered instead, once overturned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_answer: Option<String>,
}

/// The answer and author of an assumption verified as inherited; PR bodies list these.
pub(crate) const WAIVED: &str = "waived as pre-existing";
pub(crate) const WAIVER_BY: &str = "judge";
