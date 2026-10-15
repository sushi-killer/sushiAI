use crate::task::{AnsweredQuestion, Question, QuestionKind};

pub(crate) fn kind_name(kind: QuestionKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub(crate) fn option_part(options: &[String]) -> String {
    let mut options: Vec<String> = options
        .iter()
        .map(|option| option.replace(['\n', '\u{1f}'], " "))
        .collect();
    options.sort();
    options.join("\u{1f}")
}

pub(crate) enum QuestionRef<'a> {
    Open(&'a Question),
    Answered(&'a AnsweredQuestion),
}

impl QuestionRef<'_> {
    fn kind(&self) -> QuestionKind {
        match self {
            Self::Open(q) => q.kind,
            Self::Answered(q) => q.kind,
        }
    }
    fn failure_signature(&self) -> Option<&str> {
        match self {
            Self::Open(q) => q.failure_signature.as_deref(),
            Self::Answered(q) => q.failure_signature.as_deref(),
        }
    }
    fn commands(&self) -> &[String] {
        match self {
            Self::Open(q) => &q.commands,
            Self::Answered(q) => &q.commands,
        }
    }
    fn text(&self) -> &str {
        match self {
            Self::Open(q) => &q.text,
            Self::Answered(q) => &q.question,
        }
    }
    fn options(&self) -> &[String] {
        match self {
            Self::Open(q) => &q.options,
            Self::Answered(q) => &q.options,
        }
    }

    pub(crate) fn key(&self, signals: &[String]) -> String {
        let mut signals = signals.to_vec();
        signals.sort();
        let (what, options) = if is_free(self.kind()) {
            (normalized(self.text()), option_part(self.options()))
        } else {
            let mut commands: Vec<String> = self
                .commands()
                .iter()
                .map(|command| normalized(command))
                .collect();
            commands.sort();
            (commands.join("\u{1f}"), String::new())
        };
        format!(
            "{}|{}|{}|{what}|{options}",
            kind_name(self.kind()),
            self.failure_signature().unwrap_or(""),
            signals.join(",")
        )
    }
}

pub(crate) fn is_free(kind: QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::AgentQuestion | QuestionKind::PlanQuestion
    )
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
