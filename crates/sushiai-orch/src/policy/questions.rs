pub(crate) mod identity;
pub(crate) mod limits;

use crate::task::{is_option, Question, QuestionKind};

/// Kinds only the owner may allow: never the policy, the chat agent or an
/// MCP client (see [`is_refusal`] for what those may still answer).
pub(crate) fn owner_only(kind: QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::PrCheck
            | QuestionKind::Budget
            | QuestionKind::DailyBudget
            | QuestionKind::HarnessMissing
            | QuestionKind::ProtectedPath
            | QuestionKind::Permission
    )
}

/// Whether `answer` only asks the waiting loop to look again at a harness or
/// a daily budget that may have cleared; it allows nothing, so a non-owner
/// may give it.
pub(crate) fn is_recheck(question: &Question, answer: &str) -> bool {
    matches!(
        question.kind,
        QuestionKind::HarnessMissing | QuestionKind::DailyBudget
    ) && answer.trim().eq_ignore_ascii_case("retry")
}

/// Whether `answer` only denies or stops, so a non-owner may give it.
pub(crate) fn is_refusal(question: &Question, answer: &str) -> bool {
    if answer.trim().eq_ignore_ascii_case("stop") {
        return true;
    }
    match question.kind {
        QuestionKind::Permission => {
            matches!(
                crate::policy::permissions::parse_choice(answer),
                crate::policy::permissions::Choice::Deny(_)
            )
        }
        QuestionKind::ProtectedPath => is_option(answer, "reject"),
        _ => false,
    }
}

/// The option of a plan or agent question that is phrased as the cautious or
/// reversible choice.
pub(crate) fn cautious_option(options: &[String]) -> Option<&String> {
    options.iter().find(|o| {
        let o = o.to_ascii_lowercase();
        let negated = |word: &str| {
            o.contains(&format!("not {word}"))
                || o.contains(&format!("non-{word}"))
                || o.contains(&format!("in{word}"))
                || o.contains(&format!("ir{word}"))
        };
        (o.contains("cautious") && !negated("cautious"))
            || (o.contains("reversible") && !negated("reversible"))
    })
}
