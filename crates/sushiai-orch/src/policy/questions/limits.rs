use crate::policy::questions::owner_only;
use crate::task::{is_option, Question, QuestionKind, Risk};

/// What the overseer may never decide, whatever the precedent: the question
/// always goes to the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardLimit {
    /// Permission, protected path and budget questions. The standing grant is
    /// the owner's own `Always for this repo` answer.
    OwnerOnlyKind,
    /// Dropping a check or criterion.
    SharedCheckDrop,
    /// Loosening the sandbox, or anything about secrets and security.
    SandboxOrSecurity,
    /// Pushing, a pull request, merging or releasing.
    PushPrMergeRelease,
    /// A paid service or spend.
    Money,
    /// Deleting or overwriting.
    Destructive,
    /// A production deploy.
    ProductionDeploy,
    /// A change of the task's scope against the request.
    ScopeChange,
    /// A public API or contract change.
    PublicApi,
}

impl Risk {
    /// The limit a question tagged with this risk is under.
    fn limit(self) -> HardLimit {
        match self {
            Risk::Push => HardLimit::PushPrMergeRelease,
            Risk::Money => HardLimit::Money,
            Risk::Destructive => HardLimit::Destructive,
            Risk::Security => HardLimit::SandboxOrSecurity,
            Risk::Deploy => HardLimit::ProductionDeploy,
            Risk::Scope => HardLimit::ScopeChange,
            Risk::PublicApi => HardLimit::PublicApi,
        }
    }
}

/// Phrases (lower-case words) that put a question under a limit. Matching is
/// per whole word, so `pr` never matches `improve`.
const KEYWORDS: [(HardLimit, &[&str]); 5] = [
    (
        HardLimit::SandboxOrSecurity,
        &[
            "security",
            "secret",
            "secrets",
            "credential",
            "credentials",
            "password",
            "passwords",
            "keychain",
            "api key",
            "ssh key",
            "private key",
            "token",
            "tokens",
            "vulnerability",
            "unsandboxed",
            "disable the sandbox",
            "disable sandbox",
            "without the sandbox",
            "without sandbox",
            "sandbox off",
            "bypass",
            "allowlist",
            "permission",
            "permissions",
            "sudo",
            "chmod",
            "network access",
        ],
    ),
    (
        HardLimit::PushPrMergeRelease,
        &[
            "push",
            "pushing",
            "merge",
            "merging",
            "pull request",
            "pr",
            "release",
            "releasing",
            "publish",
            "publishing",
        ],
    ),
    (
        HardLimit::Money,
        &[
            "paid",
            "pay",
            "payment",
            "billing",
            "subscription",
            "invoice",
            "purchase",
            "buy",
            "usd",
            "budget",
            "credits",
            "spend",
            "price",
            "pricing",
        ],
    ),
    (
        HardLimit::Destructive,
        &[
            "delete",
            "deleting",
            "destroy",
            "wipe",
            "purge",
            "rm",
            "reset hard",
            "force",
            "overwrite",
            "drop table",
            "truncate",
            "irreversible",
        ],
    ),
    (
        HardLimit::ProductionDeploy,
        &[
            "deploy",
            "deploying",
            "deployment",
            "production",
            "prod",
            "go live",
            "rollout",
        ],
    ),
];

/// Lower-case words of `text` separated by single spaces, with a space at
/// both ends so a phrase matches only whole words.
fn words(text: &str) -> String {
    let spaced = text.to_lowercase().replace('$', " usd ");
    let words: Vec<&str> = spaced
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    format!(" {} ", words.join(" "))
}

/// The limit that puts `answer` to `question` with the owner, if any. A
/// base-check question is read by its header, not the check output it quotes.
pub(crate) fn hard_limit(question: &Question, answer: &str) -> Option<HardLimit> {
    if owner_only(question.kind) {
        return Some(HardLimit::OwnerOnlyKind);
    }
    let drops = match question.kind {
        QuestionKind::PreexistingFailure => is_option(answer, "drop all"),
        QuestionKind::Impossible => answer.trim().eq_ignore_ascii_case("drop criterion"),
        _ => false,
    };
    if drops {
        return Some(HardLimit::SharedCheckDrop);
    }
    // The asking agent's own tag, on top of the words below: it can only add a limit.
    if let Some(risk) = question.risk.first() {
        return Some(risk.limit());
    }
    let prose = if question.kind == QuestionKind::PreexistingFailure {
        crate::task::notes::overseer::question_prose(question)
    } else {
        question.text.clone()
    };
    let text = words(&format!("{prose} {} {answer}", question.options.join(" ")));
    KEYWORDS
        .iter()
        .find(|(_, phrases)| {
            phrases
                .iter()
                .any(|phrase| text.contains(&format!(" {phrase} ")))
        })
        .map(|(limit, _)| *limit)
}
