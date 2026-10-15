use crate::task::{is_option, BaseAnswer, QuestionKind, TaskData, PRE_EXISTING_RETRY};

/// Version 2 of the loop state: the kept checks, base-check answers, waited
/// out bases and answered permissions that version 1 kept in decision lines
/// and assumption texts. Like version 1, the only reader of those formats.
pub(super) fn migrate_base_and_permission_state(task: &mut TaskData) {
    if task.loop_state.state_version >= 2 {
        return;
    }
    let mut kept_checks: Vec<String> = Vec::new();
    let mut base_answers: Vec<BaseAnswer> = Vec::new();
    let mut waited_out_bases: Vec<String> = Vec::new();
    let mut answered_permissions: Vec<String> = Vec::new();
    let mut push_answer = |answer: BaseAnswer| {
        if !base_answers.contains(&answer) {
            base_answers.push(answer);
        }
    };
    for d in &task.decisions {
        if let Some(rest) = d.strip_prefix("Orchestrator: kept check ") {
            if let Some((command, _reason)) = rest.rsplit_once(", ") {
                if !kept_checks.iter().any(|c| c == command) {
                    kept_checks.push(command.to_string());
                }
            }
        } else if let Some(rest) = d.strip_prefix("Orchestrator: waited out base ") {
            if let Some(sha) = rest.strip_suffix("; not waiting for it to move again") {
                waited_out_bases.push(sha.to_string());
            }
        } else if let Some(rest) = d.strip_prefix("Orchestrator: retry base check ") {
            if let Some((command, rest)) = rest.rsplit_once(" after ") {
                if let Some(sha) = rest.strip_suffix(" moves") {
                    push_answer(BaseAnswer {
                        command: command.to_string(),
                        sha: sha.to_string(),
                        by: String::new(),
                        retry: true,
                    });
                }
            }
        } else if let Some(name) = [
            "Permission: allowed once: ",
            "Permission: allowed for this repo: ",
            "Permission: denied: ",
        ]
        .iter()
        .find_map(|p| d.strip_prefix(p))
        {
            if !answered_permissions.iter().any(|n| n == name) {
                answered_permissions.push(name.to_string());
            }
        } else if let Some((who, rest)) = d.split_once(": base check ") {
            if !matches!(who, "Owner" | "Orchestrator" | "Lead") {
                continue;
            }
            let Some((head, answer)) = rest.split_once(" -> ") else {
                continue;
            };
            let Some((command, sha)) = head.rsplit_once(" on ") else {
                continue;
            };
            if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            push_answer(BaseAnswer {
                command: command.to_string(),
                sha: sha.to_string(),
                by: who.to_string(),
                retry: is_option(answer, PRE_EXISTING_RETRY),
            });
        }
    }
    for a in &task.assumptions {
        if a.kind != Some(QuestionKind::PreexistingFailure) || a.overturned {
            continue;
        }
        if !matches!(a.by.as_str(), "policy" | "judge") {
            continue;
        }
        let first_line = a.question.lines().next().unwrap_or_default();
        let sha = first_line
            .split_once(" on base ")
            .map(|(_, rest)| rest.split(':').next().unwrap_or_default())
            .unwrap_or_default();
        for command in &a.commands {
            push_answer(BaseAnswer {
                command: command.clone(),
                sha: sha.to_string(),
                by: a.by.clone(),
                retry: is_option(&a.answer, PRE_EXISTING_RETRY),
            });
        }
    }
    let state = &mut task.loop_state;
    state.state_version = 2;
    state.kept_checks = kept_checks;
    state.base_answers = base_answers;
    state.waited_out_bases = waited_out_bases;
    state.answered_permissions = answered_permissions;
}

/// What a base-check question text quoted for `command`.
pub(super) fn legacy_base_tail(text: &str, command: &str) -> String {
    let listed = format!("- {command}: ");
    text.lines()
        .find_map(|line| {
            line.strip_prefix(&listed).map(str::to_string).or_else(|| {
                line.strip_prefix(&format!("{command} already fails on base "))
                    .and_then(|rest| rest.split_once(": ").map(|(_, tail)| tail.to_string()))
            })
        })
        .unwrap_or_default()
}
