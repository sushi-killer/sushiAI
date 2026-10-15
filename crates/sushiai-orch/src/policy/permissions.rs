use crate::task::is_option;

pub(crate) const ALLOW_ONCE: &str = "Allow once";

pub(crate) const ALLOW_ALWAYS: &str = "Always for this repo";

pub(crate) const DENY: &str = "Deny";

pub(crate) const OPTIONS: [&str; 3] = [ALLOW_ONCE, ALLOW_ALWAYS, DENY];

pub(crate) const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

pub(crate) const WEB_TOOLS: [&str; 2] = ["WebFetch", "WebSearch"];

pub(crate) fn rule_covers(rule: &str, wanted: &str) -> bool {
    rule == wanted
        || rule
            .strip_suffix("/**")
            .is_some_and(|dir| wanted.starts_with(&format!("{dir}/")))
}

pub(crate) fn granted(rules: &[String], wanted: &str) -> bool {
    rules.iter().any(|r| rule_covers(r, wanted))
}

/// What a call is about: a repo-relative path or a tool name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Subject {
    Path(String),
    Tool(String),
}

/// `a/./b/../c` without touching the disk.
pub(crate) fn normalized(rel: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

pub(crate) fn is_protected(rel: &str) -> bool {
    rel == ".claude" || rel.starts_with(".claude/")
}

/// The owner's answer to a permission question.
#[derive(Debug, PartialEq)]
pub(crate) enum Choice {
    Once,
    Always,
    Deny(Option<String>),
}

pub(crate) fn parse_choice(answer: &str) -> Choice {
    if is_option(answer, ALLOW_ONCE) {
        Choice::Once
    } else if is_option(answer, ALLOW_ALWAYS) {
        Choice::Always
    } else if is_option(answer, DENY) {
        Choice::Deny(None)
    } else {
        Choice::Deny(Some(answer.trim().to_string()).filter(|a| !a.is_empty()))
    }
}

pub(crate) fn question_text(subjects: &[Subject]) -> String {
    let names: Vec<&str> = subjects.iter().map(Subject::name).collect();
    format!("Allow {} for this task?", names.join(", "))
}

/// The `Permission:` decision line for one answered subject.
pub(crate) fn answer_line(subject: &Subject, choice: &Choice) -> String {
    let what = match choice {
        Choice::Once => "allowed once",
        Choice::Always => "allowed for this repo",
        Choice::Deny(_) => "denied",
    };
    format!("Permission: {what}: {}", subject.name())
}

impl Subject {
    pub fn rule(&self) -> String {
        match self {
            Subject::Path(p) => format!("path:{p}"),
            Subject::Tool(t) => format!("tool:{t}"),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Subject::Path(n) | Subject::Tool(n) => n,
        }
    }
}
