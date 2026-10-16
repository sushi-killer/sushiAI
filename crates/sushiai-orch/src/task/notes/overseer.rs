use crate::task::{Question, QuestionKind};
use serde::{Deserialize, Serialize};

pub const MAX_TEXT_CHARS: usize = 200;
pub const MAX_DETAIL_CHARS: usize = 1500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub ts: i64,
    pub repo: String,
    /// `woke`, `answered`, `pr`, `asked_owner`, `digest`, `done`, `skipped` or `failed`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// The full text behind `text` (older entries have none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Decision keys this entry told the owner about, newline separated (see
    /// the module docs); older entries have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{kept}\u{2026}")
}

/// Whether a line reads like a shell command or source code, not a title.
fn looks_like_code(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    const COMMANDS: [&str; 14] = [
        "node", "npm", "npx", "cargo", "git", "python", "python3", "sh", "bash", "zsh", "$", "cd",
        "pnpm", "yarn",
    ];
    line.starts_with("```")
        || line.starts_with('$')
        || line.starts_with("./")
        || line.starts_with('/')
        || line.starts_with("--")
        || COMMANDS.contains(&first)
        || line.contains(" --")
        || line.contains("&&")
        || line.contains(" | ")
        || line.ends_with(['{', ';', '}'])
}

/// A readable one-line title for a question: its kind's label, or for a free
/// question its first line that is not a command.
pub fn question_title(kind: QuestionKind, text: &str) -> String {
    let label = match kind {
        QuestionKind::AttemptsFailing => "Attempts failing",
        QuestionKind::ReviewNoVerdict => "Review gave no verdict",
        QuestionKind::DependencyEnded => "Dependency ended",
        QuestionKind::Impossible => "Task impossible",
        QuestionKind::PreexistingFailure => "Base check failing",
        QuestionKind::Budget => "Task budget",
        QuestionKind::ProtectedPath => "Protected path",
        QuestionKind::Permission => "Permission",
        QuestionKind::ReviewDispute => "Review dispute",
        QuestionKind::PlanQuestion => "Plan question",
        QuestionKind::DailyBudget => "Daily budget",
        QuestionKind::HarnessMissing => "No agent CLI",
        QuestionKind::PrCheck => "PR checks",
        QuestionKind::AgentQuestion => "",
    };
    if !label.is_empty() {
        return label.to_string();
    }
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !looks_like_code(l))
        .map_or_else(|| "Question".to_string(), |l| cut(l, 80))
}

/// The question without the command output it quotes: a base-check question
/// reads `<command>, <command> fail on base <sha>`, the commands read from
/// the question and the short sha from the text.
pub(crate) fn question_prose(question: &Question) -> String {
    const ON_BASE: &str = " on base ";
    if question.kind == QuestionKind::PreexistingFailure {
        if let Some(at) = question.text.find(ON_BASE) {
            // The header ends where the quoted output starts.
            let rest = &question.text[at + ON_BASE.len()..];
            let end = rest.find(':').unwrap_or(rest.len());
            let sha = rest[..end].trim();
            if question.commands.is_empty() {
                return format!("{}{ON_BASE}{sha}", &question.text[..at]);
            }
            let verb = if question.commands.len() == 1 {
                "fails"
            } else {
                "fail"
            };
            return format!("{} {verb}{ON_BASE}{sha}", question.commands.join(", "));
        }
    }
    question.text.clone()
}

impl Entry {
    /// An `answered` entry: "<title> -> <answer>", with the whole question and
    /// answer as the detail.
    pub fn answered(
        repo: &str,
        task_id: &str,
        question: Option<&Question>,
        answer: &str,
        now: i64,
    ) -> Entry {
        let answer = answer.trim();
        let (kind, asked) = question.map_or((QuestionKind::AgentQuestion, ""), |q| {
            (q.kind, q.text.as_str())
        });
        let prose = question.map_or_else(String::new, question_prose);
        let title = question_title(kind, asked);
        let entry = Entry::new(
            "answered",
            repo,
            Some(task_id),
            &format!("{title} \u{2192} {answer}"),
            now,
        );
        let prose = prose.trim();
        entry.with_detail(&format!("{prose}\n\nAnswer: {answer}"))
    }

    /// Keeps the full `detail` (cut to `MAX_DETAIL_CHARS`) next to the line.
    pub fn with_detail(mut self, detail: &str) -> Entry {
        let detail = detail.trim();
        if !detail.is_empty() {
            self.detail = Some(cut(detail, MAX_DETAIL_CHARS));
        }
        self
    }

    /// Records the decision keys the entry is about (none: left unset).
    pub fn with_decision(mut self, keys: &[String]) -> Entry {
        if !keys.is_empty() {
            self.decision = Some(keys.join("\n"));
        }
        self
    }

    /// The decision keys of the entry; empty for an entry without any.
    pub fn decision_keys(&self) -> impl Iterator<Item = &str> {
        self.decision.as_deref().unwrap_or("").lines()
    }

    /// An entry whose text is cut to its first line, at most `MAX_TEXT_CHARS`.
    pub fn new(kind: &str, repo: &str, task_id: Option<&str>, text: &str, now: i64) -> Entry {
        let line = text.lines().map(str::trim).find(|l| !l.is_empty());
        let line = line.unwrap_or("");
        let text = if line.chars().count() > MAX_TEXT_CHARS {
            let cut: String = line.chars().take(MAX_TEXT_CHARS - 1).collect();
            format!("{cut}\u{2026}")
        } else {
            line.to_string()
        };
        Entry {
            ts: now,
            repo: repo.to_string(),
            kind: kind.to_string(),
            task_id: task_id.map(str::to_string),
            text,
            cost_usd: None,
            detail: None,
            decision: None,
        }
    }
}
