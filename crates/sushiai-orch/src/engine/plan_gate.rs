//! The plan gate: code checks on a planner's draft, one cross-family plan
//! review, one replan, and the merge of same-file subtasks into a checklist.

use super::*;

const REVIEW_PROMPT: &str = "You review a coding task plan before an agent works on it. Another model drafted it from the owner's request. Check:\n- missing requirement: an item of the request that no criterion proves\n- wrong approach: the plan would not achieve the goal, or breaks something in the repository\n- wrong split: subtasks that edit the same files, depend on each other unseen, or leave work out\n- untestable criterion: a criterion whose check cannot be run, or proves nothing about the product's behaviour\nYou may read files in the working directory but change nothing. Report only real problems, most serious first.\nReply with only JSON: {\"findings\": [{\"severity\": \"CRITICAL|HIGH|MEDIUM|LOW\", \"kind\": \"<missing requirement|wrong approach|wrong split|untestable criterion>\", \"text\": \"<the problem and the fix, one or two sentences>\"}]}. CRITICAL means the plan fails the request as written; use it sparingly. An empty list means the plan is sound.";

const MAX_FINDING_CHARS: usize = 400;
const MAX_REQUEST_ITEMS: usize = 12;
const MAX_ITEM_CHARS: usize = 300;

/// Sections orchd appends to a subtask's request; they are not the owner's
/// own text, so no enumerated item of theirs counts.
const APPENDED_MARKERS: [&str; 2] = [
    "This is one part of a larger task",
    "Already decided for the whole task",
];

/// What the gate tells the plan stage to do next.
pub(super) enum GateAction {
    Proceed,
    /// The planner runs once more with the findings in its brief.
    Replan,
    Cancelled,
}

/// Everything the code checks read from a stored draft.
pub(super) struct DraftView<'a> {
    pub request: &'a str,
    pub criteria: &'a [String],
    pub visual: &'a [String],
    pub checks: &'a [Check],
    pub coverage: &'a [brief::Coverage],
    pub verify: &'a [String],
    pub final_verify: &'a [String],
    pub paths: &'a [String],
    pub subtasks: &'a [brief::PlanSubtask],
    pub worktree: &'a Path,
}

fn finding(kind: &str, n: usize, severity: Severity, text: String) -> PlanFinding {
    PlanFinding {
        id: format!("{kind}-{n}"),
        severity,
        kind: kind.to_string(),
        text: truncate_chars(&text, MAX_FINDING_CHARS),
    }
}

/// The owner's own words: the request up to the first section orchd added.
pub(super) fn owner_text(request: &str) -> &str {
    let end = APPENDED_MARKERS
        .iter()
        .filter_map(|m| request.find(m))
        .min()
        .unwrap_or(request.len());
    &request[..end]
}

/// The criterion's own text and the text after its `-- check:` marker.
fn split_check(criterion: &str) -> (&str, Option<&str>) {
    match criterion.to_ascii_lowercase().find("-- check:") {
        Some(at) => (
            criterion[..at].trim(),
            Some(criterion[at + "-- check:".len()..].trim()),
        ),
        None => (criterion.trim(), None),
    }
}

/// Whether a criterion is about the agent's own report, handoff, summary,
/// decisions or process rather than the product's behaviour.
fn is_report_only(criterion: &str) -> bool {
    let body = split_check(criterion).0.to_lowercase();
    let body = body.trim();
    let about_report = [
        "report",
        "handoff",
        "hand-off",
        "hand off",
        "summary",
        "decisions",
        "final message",
    ]
    .iter()
    .any(|w| body.contains(w));
    let about_process = [
        "process",
        "explains",
        "explain ",
        "mentions",
        "states that",
        "describes",
        "notes that",
        "justif",
    ]
    .iter()
    .any(|w| body.contains(w));
    let agent = [
        "implementer",
        "the agent",
        "task agent",
        "the assistant",
        "your report",
        "your handoff",
        "your summary",
        "your final",
    ]
    .iter()
    .any(|w| body.contains(w));
    // `The report lists ...` is about the agent's report; `The report page
    // shows ...` is product behaviour.
    let verbs = [
        "states",
        "lists",
        "names",
        "says",
        "mentions",
        "explains",
        "describes",
        "notes",
        "includes",
        "contains",
        "records",
        "documents",
        "confirms",
    ];
    let starts_with_report = [
        "the report ",
        "the summary ",
        "the handoff ",
        "the final message ",
        "the decisions ",
    ]
    .iter()
    .any(|w| {
        body.strip_prefix(w)
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|word| verbs.contains(&word))
    });
    (agent && (about_report || about_process)) || starts_with_report
}

const COMMANDS: [&str; 24] = [
    "cargo", "npm", "pnpm", "yarn", "npx", "node", "sh", "bash", "zsh", "python", "python3",
    "pytest", "grep", "rg", "jq", "curl", "tsc", "eslint", "prettier", "cat", "ls", "wc", "deno",
    "bun",
];

fn strip_wrapping(token: &str) -> &str {
    token.trim_matches(|c| matches!(c, '`' | '"' | '\'' | '$' | '(' | ')' | ',' | '.'))
}

fn segment_is_command(segment: &str) -> bool {
    let mut tokens: Vec<&str> = segment.split_whitespace().collect();
    while let Some(first) = tokens.first().map(|t| strip_wrapping(t).to_lowercase()) {
        if matches!(
            first.as_str(),
            "run"
                | "running"
                | "execute"
                | "executing"
                | "the"
                | "a"
                | "command"
                | "via"
                | "with"
                | "using"
                | "`"
                | "$"
                | ""
        ) || tokens[0].contains('=') && !tokens[0].contains('/')
        {
            tokens.remove(0);
        } else {
            break;
        }
    }
    let Some(first) = tokens.first().map(|t| strip_wrapping(t)) else {
        return false;
    };
    let second = tokens.get(1).map(|t| strip_wrapping(t)).unwrap_or("");
    let lower = first.to_lowercase();
    if COMMANDS.contains(&lower.as_str()) || first.starts_with("./") {
        return true;
    }
    match lower.as_str() {
        "make" => !second.is_empty() && !matches!(second, "sure" | "the" | "a" | "it" | "that"),
        "git" => matches!(
            second,
            "diff" | "log" | "show" | "status" | "grep" | "ls-files" | "rev-parse" | "cat-file"
        ),
        "go" => matches!(second, "test" | "vet" | "build"),
        "test" => second.starts_with('-'),
        "cd" => segment.contains("&&"),
        _ => false,
    }
}

fn shell_segments(text: &str) -> Vec<String> {
    text.replace("&&", "\n")
        .replace("||", "\n")
        .replace([';', '|'], "\n")
        .replace(" then ", "\n")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Whether a `-- check:` names a command to run or a test to run.
fn names_command(check: &str) -> bool {
    let spans = check
        .split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string());
    spans
        .chain(std::iter::once(check.to_string()))
        .any(|c| shell_segments(&c).iter().any(|s| segment_is_command(s)))
        || check.split_whitespace().any(|t| {
            let t = strip_wrapping(t);
            t.contains(".test.")
                || t.contains("::")
                || t.ends_with("_test.rs")
                || t.contains("tests/")
        })
}

/// A criterion proven by an image, not by a command.
fn is_visual(criterion: &str, visual: &[String]) -> bool {
    visual.iter().any(|v| v == criterion)
        || split_check(criterion)
            .1
            .is_some_and(|c| c.contains("artifacts/"))
}

/// Whether criterion `i` has something orchd can run or look at.
fn has_check(i: usize, v: &DraftView) -> bool {
    let Some(criterion) = v.criteria.get(i) else {
        return false;
    };
    is_visual(criterion, v.visual)
        || v.checks
            .iter()
            .any(|c| c.criterion == i && !c.run.trim().is_empty())
        || split_check(criterion).1.is_some_and(names_command)
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect()
}

/// The enumerated items of the owner's request: `1.` / `1)` / `a.` / `-` lines.
fn request_items(request: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in owner_text(request).lines() {
        let line = line.trim();
        let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
        let rest = if digits > 0 {
            line[digits..]
                .strip_prefix(['.', ')'])
                .and_then(|r| r.strip_prefix(' '))
        } else if line.len() > 2
            && line.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && matches!(line.as_bytes()[1], b'.' | b')')
            && line.as_bytes()[2] == b' '
        {
            Some(&line[3..])
        } else {
            line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
        };
        if let Some(rest) = rest.map(str::trim).filter(|r| !r.is_empty()) {
            items.push(rest.to_string());
        }
        if items.len() >= MAX_REQUEST_ITEMS {
            break;
        }
    }
    items
}

/// Whether a coverage entry's requirement stands for the request item.
fn same_requirement(item: &str, requirement: &str) -> bool {
    let (a, b) = (words(item), words(requirement));
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    let (ja, jb) = (a.join(" "), b.join(" "));
    ja.contains(&jb)
        || jb.contains(&ja)
        || a.iter().filter(|w| b.contains(w)).count() * 10 >= a.len() * 6
}

/// A backticked span of the owner's text that is a shared check.
fn named_shared_checks(request: &str, shared: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, span) in owner_text(request).split('`').enumerate() {
        let span = span.split_whitespace().collect::<Vec<_>>().join(" ");
        if i % 2 == 1 && !span.is_empty() && shared(&span) && !out.contains(&span) {
            out.push(span);
        }
    }
    out
}

fn flat(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn path_allows(paths: &[String], rel: &str) -> bool {
    let rel = rel.trim_start_matches("./").trim_end_matches('/');
    paths.iter().any(|p| {
        let p = p.trim().trim_start_matches("./").trim_end_matches('/');
        !p.is_empty() && (rel == p || rel.starts_with(&format!("{p}/")))
    })
}

fn skipped_dir(name: &str) -> bool {
    matches!(
        name,
        "target" | "node_modules" | ".git" | ".sushiai" | "dist"
    )
}

/// Files named `file_name` in `root` or up to `depth` directories below it.
fn find_files(root: &Path, file_name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if depth > 0 && !skipped_dir(&name) {
                find_files(&path, file_name, depth - 1, out);
            }
        } else if name == file_name {
            out.push(path);
        }
    }
}

fn exists_below(root: &Path, relative: &str) -> bool {
    let (dir, file) = relative.rsplit_once('/').unwrap_or(("", relative));
    let mut found = Vec::new();
    find_files(root, file, 4, &mut found);
    found.iter().any(|p| {
        p.parent()
            .is_some_and(|d| d.ends_with(dir) || dir.is_empty())
    })
}

fn package_scripts(dir: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(
        v.get("scripts")
            .and_then(|s| s.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default(),
    )
}

fn crate_exists(root: &Path, name: &str) -> bool {
    let mut manifests = Vec::new();
    find_files(root, "Cargo.toml", 4, &mut manifests);
    if manifests.is_empty() {
        return true;
    }
    let needle = format!("name = \"{name}\"");
    manifests.iter().any(|m| {
        std::fs::read_to_string(m)
            .map(|t| t.contains(&needle))
            .unwrap_or(false)
    })
}

const FILE_COMMANDS: [&str; 17] = [
    "node", "sh", "bash", "zsh", "python", "python3", "pytest", "grep", "rg", "cat", "wc", "diff",
    "jq", "test", "ls", "head", "tail",
];
const FILE_EXTENSIONS: [&str; 12] = [
    ".rs", ".ts", ".tsx", ".js", ".cjs", ".mjs", ".json", ".md", ".sh", ".toml", ".py", ".css",
];

/// What a verify command names that the repository does not have.
fn missing_in(root: &Path, command: &str, paths: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    let mut cwd = root.to_path_buf();
    for segment in shell_segments(command) {
        let mut tokens: Vec<&str> = segment.split_whitespace().collect();
        while matches!(tokens.first(), Some(t) if t.contains('=') && !t.starts_with('-'))
            || matches!(tokens.first(), Some(&"env"))
        {
            tokens.remove(0);
        }
        let Some(&first) = tokens.first() else {
            continue;
        };
        if first == "cd" {
            if let Some(dir) = tokens
                .get(1)
                .filter(|d| !d.starts_with(['/', '~', '$', '-']))
            {
                cwd = root.join(dir);
            }
            continue;
        }
        let rel_cwd = cwd
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let from_root = |name: &str| {
            if rel_cwd.is_empty() {
                name.to_string()
            } else {
                format!("{rel_cwd}/{name}")
            }
        };
        match first {
            "npm" | "pnpm" | "yarn" => {
                let script = match (tokens.get(1).copied(), tokens.get(2).copied()) {
                    (Some("run" | "run-script"), Some(s)) => Some(s),
                    (Some("test"), _) => Some("test"),
                    _ => None,
                };
                if let (Some(script), Some(scripts)) = (script, package_scripts(&cwd)) {
                    if !scripts.iter().any(|s| s == script)
                        && !path_allows(paths, &from_root("package.json"))
                    {
                        missing.push(format!("the script `{script}` is not in package.json"));
                    }
                }
            }
            "cargo" if tokens.get(1) == Some(&"test") => {
                let mut i = 2;
                while i < tokens.len() {
                    let t = tokens[i];
                    let value = tokens.get(i + 1).copied();
                    match (t, value) {
                        ("--test", Some(name)) => {
                            let tests_dir = format!("tests/{name}");
                            let found = exists_below(&cwd, &format!("{tests_dir}.rs"))
                                || exists_below(&cwd, &format!("{tests_dir}/main.rs"));
                            if !found && !paths.iter().any(|p| p.contains(name)) {
                                missing
                                    .push(format!("the cargo test target `{name}` does not exist"));
                            }
                            i += 1;
                        }
                        ("-p" | "--package", Some(name)) => {
                            if !crate_exists(&cwd, name) && !paths.iter().any(|p| p.contains(name))
                            {
                                missing.push(format!("the cargo package `{name}` does not exist"));
                            }
                            i += 1;
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        if FILE_COMMANDS.contains(&first) || first.starts_with("./") {
            let skip_pattern = matches!(first, "grep" | "rg" | "jq");
            let mut positional = 0;
            let mut redirected = false;
            for (n, token) in tokens
                .iter()
                .enumerate()
                .skip(usize::from(!first.starts_with("./")))
            {
                if redirected {
                    redirected = false;
                    continue;
                }
                if token.starts_with('>') {
                    redirected = *token == ">" || *token == ">>";
                    continue;
                }
                if token.starts_with('-') || token.starts_with(['\'', '"', '/', '~', '$']) {
                    continue;
                }
                positional += 1;
                if skip_pattern && positional == 1 {
                    continue;
                }
                let token = token.trim_end_matches([',', ';', ')']);
                if token.contains(['*', '?', '{', '$', '<', '=', ':']) || n == 0 {
                    continue;
                }
                let looks_like_file =
                    token.contains('/') || FILE_EXTENSIONS.iter().any(|e| token.ends_with(e));
                if !looks_like_file {
                    continue;
                }
                let rel = from_root(token);
                if !cwd.join(token).exists() && !path_allows(paths, &rel) {
                    missing.push(format!("the file `{token}` does not exist"));
                }
            }
        }
        if first.starts_with("./") {
            let rel = from_root(first);
            if !cwd.join(first).exists() && !path_allows(paths, &rel) {
                missing.push(format!("the file `{first}` does not exist"));
            }
        }
    }
    missing.dedup();
    missing
}

/// The code checks of the plan gate: pure over a stored draft.
pub(super) fn code_checks(v: &DraftView, shared: &dyn Fn(&str) -> bool) -> Vec<PlanFinding> {
    let mut out = Vec::new();
    for (i, c) in v.criteria.iter().enumerate() {
        if is_report_only(c) {
            out.push(finding(
                "report-only",
                i,
                Severity::Critical,
                format!(
                    "Criterion {i} is about the agent's report, handoff, summary or process, not the product's behaviour: \"{}\"",
                    truncate_chars(split_check(c).0, 160)
                ),
            ));
        }
    }
    let reported: Vec<usize> = out
        .iter()
        .filter_map(|f| f.id.rsplit('-').next()?.parse().ok())
        .collect();
    for (n, item) in request_items(v.request).iter().enumerate() {
        let matched: Vec<&brief::Coverage> = v
            .coverage
            .iter()
            .filter(|c| same_requirement(item, &c.requirement))
            .collect();
        let item_text = truncate_chars(item, 160);
        if matched.is_empty() {
            out.push(finding(
                "coverage",
                n,
                Severity::Critical,
                format!(
                    "Request item \"{item_text}\" is in no coverage entry: no criterion proves it"
                ),
            ));
            continue;
        }
        let proven = matched.iter().any(|c| {
            c.criteria
                .iter()
                .any(|&i| !reported.contains(&i) && has_check(i, v))
        });
        if !proven {
            out.push(finding(
                "coverage",
                n,
                Severity::Critical,
                format!(
                    "Request item \"{item_text}\" maps only to criteria with no runnable check"
                ),
            ));
        }
    }
    for (n, command) in named_shared_checks(v.request, shared).iter().enumerate() {
        let wanted = flat(command);
        let kept = v
            .verify
            .iter()
            .chain(v.final_verify)
            .any(|c| flat(c).contains(&wanted));
        if !kept {
            out.push(finding(
                "shared-check",
                n,
                Severity::Critical,
                format!(
                    "The request names the shared check `{command}` but neither verify nor finalVerify runs it"
                ),
            ));
        }
    }
    for (i, c) in v.criteria.iter().enumerate() {
        if is_visual(c, v.visual) || reported.contains(&i) {
            continue;
        }
        if !has_check(i, v) {
            out.push(finding(
                "untestable",
                i,
                Severity::High,
                format!(
                    "Criterion {i} has no `-- check:` that names a runnable command or test: \"{}\"",
                    truncate_chars(c, 160)
                ),
            ));
        }
    }
    let mut n = 0;
    for command in v.verify.iter().chain(v.final_verify) {
        for what in missing_in(v.worktree, command, v.paths) {
            out.push(finding(
                "verify-missing",
                n,
                Severity::High,
                format!("`{}`: {what}", truncate_chars(command, 160)),
            ));
            n += 1;
        }
    }
    for (n, command) in v
        .verify
        .iter()
        .filter(|c| whole_repo_suite(v.worktree, c))
        .enumerate()
    {
        out.push(finding(
            "verify-cost",
            n,
            Severity::Medium,
            format!(
                "`{}` runs the whole repository after every attempt",
                truncate_chars(command, 160)
            ),
        ));
    }
    let mut n = 0;
    for (i, a) in v.subtasks.iter().enumerate() {
        for b in &v.subtasks[i + 1..] {
            if let Some(shared) = overlapping_path(&a.paths, &b.paths) {
                out.push(finding(
                    "overlap",
                    n,
                    Severity::Low,
                    format!(
                        "Subtasks {} and {} both touch {shared}",
                        a.key.trim(),
                        b.key.trim()
                    ),
                ));
                n += 1;
            }
        }
    }
    out
}

/// Reads the review's JSON: first `{` to last `}`, severity in any case
/// (unknown = MEDIUM), entries without text dropped. `Err` is an unusable
/// reply.
pub(super) fn parse_review_findings(text: &str) -> Result<Vec<PlanFinding>, String> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err("the reply had no JSON".to_string());
    };
    let v: serde_json::Value = serde_json::from_str(&text[start..=end])
        .map_err(|_| "the reply was not valid JSON".to_string())?;
    let Some(list) = v.get("findings").and_then(|f| f.as_array()) else {
        return Err("the reply had no findings list".to_string());
    };
    let mut out = Vec::new();
    for entry in list {
        let body = entry
            .get("text")
            .and_then(|t| t.as_str())
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        if body.is_empty() {
            continue;
        }
        let severity = match entry
            .get("severity")
            .and_then(|s| s.as_str())
            .map(|s| s.trim().to_uppercase())
            .as_deref()
        {
            Some("CRITICAL") => Severity::Critical,
            Some("HIGH") => Severity::High,
            Some("LOW") => Severity::Low,
            _ => Severity::Medium,
        };
        let kind = entry
            .get("kind")
            .and_then(|k| k.as_str())
            .map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "review".to_string());
        out.push(PlanFinding {
            id: format!("review-{}", out.len()),
            severity,
            kind: truncate_chars(&kind, 40),
            text: truncate_chars(&body, MAX_FINDING_CHARS),
        });
    }
    Ok(out)
}

fn build_review_prompt(task: &Task, request: &str, subtasks: &[brief::PlanSubtask]) -> String {
    let parts: Vec<serde_json::Value> = subtasks
        .iter()
        .map(|s| {
            json!({"key": s.key, "title": s.title, "request": s.request,
                   "goal": s.goal, "criteria": s.criteria, "verify": s.verify,
                   "finalVerify": s.final_verify, "checks": s.checks,
                   "heldOutCriterion": s.held_out.as_ref().map(|h| h.criterion), "tests": s.tests,
                   "visualCriteria": s.visual_criteria, "screenshot": s.screenshot,
                   "tier": s.tier, "dependsOn": s.depends_on,
                   "paths": s.paths, "check": s.check_command()})
        })
        .collect();
    format!(
        "{REVIEW_PROMPT}\n\n{}\n",
        json!({
            "request": request,
            "goal": task.goal,
            "criteria": task.criteria,
            "checks": task.checks,
            "heldOutCriterion": task.held_out.as_ref().map(|h| h.criterion),
            "tests": task.planner_tests.files,
            "verify": task.verify,
            "finalVerify": task.final_verify,
            "paths": task.paths,
            "subtasks": parts,
        })
    )
}

/// The `## Plan gate findings` block of a replan brief.
pub(super) fn findings_block(findings: &[PlanFinding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Plan gate findings\n\nThe previous draft of this plan was checked and these problems were found. Draft the plan again so that every CRITICAL one is resolved; the request itself does not change.\n\n",
    );
    for f in findings {
        out.push_str(&format!(
            "- {} ({}): {}\n",
            f.severity.label(),
            f.kind,
            f.text
        ));
    }
    out.push('\n');
    out
}

enum ReviewRoute {
    Off,
    Skipped(String),
    Route(Route),
}

/// The plan review's route: exactly the one named, on an available harness.
/// There is no fallback because the review must be another model family.
fn review_route(app: &App) -> ReviewRoute {
    let settings = app.settings.read().unwrap();
    let id = settings.plan_review.trim().to_string();
    if id.is_empty() {
        return ReviewRoute::Off;
    }
    match settings.routes.iter().find(|r| r.id == id) {
        None => ReviewRoute::Skipped(format!("route {id} is not configured")),
        Some(r) if !app.harness_available(r.harness) => {
            ReviewRoute::Skipped(format!("the agent CLI of route {id} is not available"))
        }
        Some(r) => ReviewRoute::Route(r.clone()),
    }
}

fn criteria_of(findings: &[PlanFinding], kind: &str) -> Vec<usize> {
    findings
        .iter()
        .filter(|f| f.kind == kind)
        .filter_map(|f| f.id.rsplit('-').next()?.parse().ok())
        .collect()
}

/// Drops the criteria at `remove` and everything that points at an index:
/// grounded checks are removed or reindexed, visual marks go by text.
fn remove_criteria(task: &mut Task, remove: &[usize]) {
    let shift = |i: usize| i - remove.iter().filter(|&&r| r < i).count();
    let removed_texts: Vec<String> = remove
        .iter()
        .filter_map(|&i| task.criteria.get(i).cloned())
        .collect();
    task.checks = std::mem::take(&mut task.checks)
        .into_iter()
        .filter(|c| !remove.contains(&c.criterion))
        .map(|mut c| {
            c.criterion = shift(c.criterion);
            c
        })
        .collect();
    task.held_out = task.held_out.take().and_then(|mut c| {
        (!remove.contains(&c.criterion)).then(|| {
            c.criterion = shift(c.criterion);
            c
        })
    });
    task.visual_criteria.retain(|v| !removed_texts.contains(v));
    task.criteria = std::mem::take(&mut task.criteria)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !remove.contains(i))
        .map(|(_, c)| c)
        .collect();
}

fn is_critical(f: &PlanFinding) -> bool {
    f.severity == Severity::Critical
}

/// Runs the gate on the draft just stored on the task: code checks, the one
/// plan review; then the one replan if anything is
/// critical, otherwise the findings are stored for the implementer.
pub(super) async fn run(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    draft: &brief::PlanDraft,
    cancel: &CancelToken,
) -> GateAction {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return GateAction::Proceed;
    };
    let settings = app.settings.read().unwrap().clone();
    let request = task.request.clone().unwrap_or_default();
    let first_pass = !task.plan_gate.reviewed;
    let carried: Vec<PlanFinding> = task
        .plan_gate
        .findings
        .iter()
        .filter(|f| f.id.starts_with("review-") && !is_critical(f))
        .cloned()
        .collect();

    // The plan review: another model family, once per task.
    let mut review: Vec<PlanFinding> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    if first_pass {
        match review_route(app) {
            ReviewRoute::Off => {
                lines.push("Plan gate: the plan review is off (planReview is empty)".to_string())
            }
            ReviewRoute::Skipped(why) => lines.push(format!(
                "Plan gate: plan review skipped, {why}; it needs another model family, so there is no fallback"
            )),
            ReviewRoute::Route(route) => {
                let prompt = build_review_prompt(&task, owner_text(&request), &draft.subtasks);
                match run_judge(
                    app, &task, &route, attempt_n, "plan", "plan-review", &prompt, cancel,
                )
                .await
                {
                    Err(RunError::Cancelled) => return GateAction::Cancelled,
                    Err(RunError::Io(msg) | RunError::NotFound(msg)) => lines.push(format!(
                        "Plan gate: plan review failed ({}); continuing",
                        truncate_chars(&msg, 200)
                    )),
                    Ok((reply, cost)) => {
                        add_task_cost(app, task_id, cost);
                        match reply.and_then(|text| parse_review_findings(&text)) {
                            Ok(found) => {
                                lines.push(format!(
                                    "Plan gate: plan review by {} found {} problem(s)",
                                    route.id,
                                    found.len()
                                ));
                                review = found;
                            }
                            Err(why) => lines.push(format!(
                                "Plan gate: plan review gave no usable answer ({}); continuing",
                                truncate_chars(&why, 200)
                            )),
                        }
                    }
                }
            }
        }
    } else {
        review = carried;
    }

    // Reload: the review's cost went to the stored task.
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return GateAction::Proceed;
    };
    task.decisions.extend(lines);
    if first_pass {
        task.plan_gate.reviewed = true;
    }
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);

    let replanned = task.plan_gate.replanned;
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return GateAction::Proceed;
    };

    let code = {
        let task_ref = &task;
        let is_shared = |c: &str| answer_policy::is_shared_check(&settings, task_ref, c);
        let view = DraftView {
            request: &request,
            criteria: &task.criteria,
            visual: &task.visual_criteria,
            checks: &task.checks,
            coverage: &draft.coverage,
            verify: &task.verify,
            final_verify: &task.final_verify,
            paths: &task.paths,
            subtasks: &draft.subtasks,
            worktree: Path::new(&task.worktree),
        };
        code_checks(&view, &is_shared)
    };
    let all: Vec<PlanFinding> = code.iter().chain(review.iter()).cloned().collect();

    if all.iter().any(is_critical) && !replanned {
        let kinds: Vec<String> = {
            let mut k: Vec<String> = all
                .iter()
                .filter(|f| is_critical(f))
                .map(|f| f.kind.clone())
                .collect();
            k.dedup();
            k
        };
        task.decisions.push(format!(
            "Plan gate: {} critical finding(s) ({}); planning again once",
            all.iter().filter(|f| is_critical(f)).count(),
            kinds.join(", ")
        ));
        task.plan_gate.replanned = true;
        task.plan_gate.findings = all;
        // The new draft is checked again, from scratch.
        task.brief_check.done = false;
        task.brief_check.missing_tool = None;
        task.status = TaskStatus::Drafting;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
        return GateAction::Replan;
    }

    // Final: a report-only criterion that survived the replan is removed.
    let report_only = criteria_of(&all, "report-only");
    let mut stored = all;
    if !report_only.is_empty() {
        for i in &report_only {
            if let Some(c) = task.criteria.get(*i) {
                task.decisions.push(format!(
                    "Plan gate: removed the report-only criterion \"{}\"",
                    truncate_chars(split_check(c).0, 160)
                ));
            }
        }
        remove_criteria(&mut task, &report_only);
        let task_ref = &task;
        let is_shared = |c: &str| answer_policy::is_shared_check(&settings, task_ref, c);
        let view = DraftView {
            request: &request,
            criteria: &task.criteria,
            visual: &task.visual_criteria,
            checks: &task.checks,
            coverage: &draft.coverage,
            verify: &task.verify,
            final_verify: &task.final_verify,
            paths: &task.paths,
            subtasks: &draft.subtasks,
            worktree: Path::new(&task.worktree),
        };
        stored = code_checks(&view, &is_shared);
        stored.extend(review.iter().cloned());
    }
    // Handled elsewhere: overlap by the merge. A verify-cost finding left
    // here could not be moved.
    stored.retain(|f| f.kind != "overlap");
    // A review finding the replan was asked to fix is not carried on.
    if replanned {
        stored.retain(|f| !(f.id.starts_with("review-") && is_critical(f)));
    }
    if !stored.is_empty() {
        task.decisions.push(format!(
            "Plan gate: {} finding(s) left for the implementer's brief",
            stored.len()
        ));
    }
    task.plan_gate.findings = stored;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
    GateAction::Proceed
}

/// What the plan stage does with the planner's subtasks: parts that keep
/// their own child, and the ordered parts of the one merged task.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct MergePlan {
    pub separate: Vec<usize>,
    pub merged: Vec<usize>,
}

/// A part stays its own child when it lists paths and targeted proof.
/// Overlapping complete drafts are serialised; incomplete overlaps become
/// one merged group, in planner
/// order made stable-topological by `dependsOn`. If the group, as one node,
/// would make the dependency graph cyclic, everything merges.
pub(super) fn merge_parts(parts: &[brief::PlanSubtask]) -> MergePlan {
    let keys: Vec<&str> = parts.iter().map(|p| p.key.trim()).collect();
    let stands_alone = |i: usize| {
        !parts[i].paths.is_empty()
            && !parts[i].check_command().is_empty()
            && !(0..parts.len()).any(|j| {
                j != i
                    && overlapping_path(&parts[i].paths, &parts[j].paths).is_some()
                    && !(parts[i].is_complete() && parts[j].is_complete())
            })
    };
    let mut separate: Vec<usize> = (0..parts.len()).filter(|&i| stands_alone(i)).collect();
    let mut merged: Vec<usize> = (0..parts.len()).filter(|i| !separate.contains(i)).collect();
    if !merged.is_empty() && !separate.is_empty() {
        const GROUP: &str = "\u{0}group";
        let node = |key: &str, group: &[usize]| -> String {
            match keys.iter().position(|k| *k == key) {
                Some(at) if group.contains(&at) => GROUP.to_string(),
                _ => key.to_string(),
            }
        };
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        for (i, part) in parts.iter().enumerate() {
            let from = if merged.contains(&i) {
                GROUP.to_string()
            } else {
                keys[i].to_string()
            };
            let entry = edges.entry(from.clone()).or_default();
            for d in &part.depends_on {
                let to = node(d.trim(), &merged);
                if to != from {
                    entry.push(to);
                }
            }
        }
        if has_cycle(&edges) {
            merged = (0..parts.len()).collect();
            separate.clear();
        }
    }
    // Stable topological order inside the group.
    let mut ordered: Vec<usize> = Vec::new();
    let mut left = merged.clone();
    while !left.is_empty() {
        let next = left
            .iter()
            .position(|&i| {
                parts[i].depends_on.iter().all(|d| {
                    keys.iter()
                        .position(|k| *k == d.trim())
                        .is_none_or(|at| !left.contains(&at) || at == i)
                })
            })
            .unwrap_or(0);
        ordered.push(left.remove(next));
    }
    MergePlan {
        separate,
        merged: ordered,
    }
}

/// The checklist of a merged group, in group order.
pub(super) fn checklist_of(parts: &[brief::PlanSubtask], merged: &[usize]) -> Vec<ChecklistItem> {
    merged
        .iter()
        .map(|&i| ChecklistItem {
            text: truncate_chars(parts[i].request.trim(), MAX_ITEM_CHARS),
            check: parts[i].check_command().to_string(),
            passes: false,
        })
        .collect()
}

/// Runs each checklist item's check in the worktree and records `passes`.
/// Returns the failure detail naming the failing items, or `None` when all
/// pass, there is nothing to run, or the run was cancelled.
pub(super) async fn run_checklist(
    app: &Arc<App>,
    task: &mut Task,
    worktree: &Path,
    run_dir: &Path,
    sandbox: SandboxMode,
    cancel: &CancelToken,
) -> Option<String> {
    let runnable: Vec<usize> = (0..task.checklist.len())
        .filter(|&i| !task.checklist[i].check.trim().is_empty())
        .collect();
    if runnable.is_empty() {
        return None;
    }
    let commands: Vec<String> = runnable
        .iter()
        .map(|&i| task.checklist[i].check.clone())
        .collect();
    let results = run_verify_commands(
        CheckContext::new(app, Some(&task.id)),
        worktree,
        run_dir,
        &commands,
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await;
    if cancel.is_cancelled() {
        return None;
    }
    let mut failed = Vec::new();
    for (&i, result) in runnable.iter().zip(&results) {
        task.checklist[i].passes = result.code == Some(0);
        if !task.checklist[i].passes {
            failed.push(format!(
                "Checklist item \"{}\": `{}` exited {}.\n{}",
                truncate_chars(&task.checklist[i].text, 120),
                result.command,
                result
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                result.tail
            ));
        }
    }
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    (!failed.is_empty()).then(|| failed.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(
        request: &'a str,
        criteria: &'a [String],
        verify: &'a [String],
        root: &'a Path,
    ) -> DraftView<'a> {
        DraftView {
            request,
            criteria,
            visual: &[],
            checks: &[],
            coverage: &[],
            verify,
            final_verify: &[],
            paths: &[],
            subtasks: &[],
            worktree: root,
        }
    }

    fn kinds(findings: &[PlanFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.kind.as_str()).collect()
    }

    fn part(key: &str, deps: &[&str], paths: &[&str], check: &str) -> brief::PlanSubtask {
        brief::PlanSubtask {
            key: key.into(),
            title: key.to_uppercase(),
            request: format!("do {key}"),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            check: check.into(),
            ..Default::default()
        }
    }

    #[test]
    fn plan_gate_flags_a_report_only_criterion_but_not_a_product_report() {
        let root = tempfile::tempdir().unwrap();
        let criteria = vec![
            "The implementer's handoff lists the changed files".to_string(),
            "orchd's task report shows a Metrics line -- check: cargo test -p sushiai-orch --lib metrics".to_string(),
        ];
        let found = code_checks(&view("", &criteria, &[], root.path()), &|_| false);
        let report: Vec<&PlanFinding> = found.iter().filter(|f| f.kind == "report-only").collect();
        assert_eq!(report.len(), 1, "{found:?}");
        assert_eq!(report[0].severity, Severity::Critical);
        assert_eq!(report[0].id, "report-only-0");
    }

    #[test]
    fn plan_gate_flags_an_uncovered_numbered_request_item() {
        let root = tempfile::tempdir().unwrap();
        let criteria = vec!["Toggle exists -- check: cargo test -p x --lib toggle".to_string()];
        let request = "Do these:\n1. add a toggle\n2. persist the choice\n\nThis is one part of a larger task, \"X\": y\n\nAlready decided for the whole task:\n- Owner: dark -> yes\n";
        let coverage = vec![brief::Coverage {
            requirement: "add a toggle".into(),
            criteria: vec![0],
        }];
        let mut v = view(request, &criteria, &[], root.path());
        v.coverage = &coverage;
        let found = code_checks(&v, &|_| false);
        let cov: Vec<&PlanFinding> = found.iter().filter(|f| f.kind == "coverage").collect();
        // Item 2 is uncovered; the `Already decided` bullet never counts.
        assert_eq!(cov.len(), 1, "{found:?}");
        assert!(cov[0].text.contains("persist the choice"));
        assert_eq!(cov[0].severity, Severity::Critical);
    }

    #[test]
    fn plan_gate_flags_a_coverage_item_mapped_only_to_an_unchecked_criterion() {
        let root = tempfile::tempdir().unwrap();
        let criteria = vec!["The toggle works".to_string()];
        let coverage = vec![brief::Coverage {
            requirement: "add a toggle".into(),
            criteria: vec![0],
        }];
        let mut v = view("1. add a toggle", &criteria, &[], root.path());
        v.coverage = &coverage;
        let found = code_checks(&v, &|_| false);
        assert!(kinds(&found).contains(&"coverage"), "{found:?}");
        assert!(kinds(&found).contains(&"untestable"), "{found:?}");
    }

    #[test]
    fn plan_gate_flags_a_check_that_names_no_command() {
        let root = tempfile::tempdir().unwrap();
        let criteria = vec![
            "The toggle is visible -- check: look at the settings screen".to_string(),
            "Saved -- check: cargo test -p x --lib saved".to_string(),
        ];
        let found = code_checks(&view("", &criteria, &[], root.path()), &|_| false);
        let bad: Vec<&PlanFinding> = found.iter().filter(|f| f.kind == "untestable").collect();
        assert_eq!(bad.len(), 1, "{found:?}");
        assert_eq!(bad[0].id, "untestable-0");
        assert_eq!(bad[0].severity, Severity::High);
    }

    #[test]
    fn plan_gate_exempts_a_visual_criterion_from_the_check_rule() {
        let root = tempfile::tempdir().unwrap();
        let criteria = vec!["The screen looks right".to_string()];
        let visual = criteria.clone();
        let mut v = view("", &criteria, &[], root.path());
        v.visual = &visual;
        assert!(!kinds(&code_checks(&v, &|_| false)).contains(&"untestable"));
    }

    #[test]
    fn plan_gate_flags_a_missing_npm_script_and_cargo_test_target() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"build":"tsc"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join("crates/x/tests")).unwrap();
        std::fs::write(
            root.path().join("crates/x/Cargo.toml"),
            "[package]\nname = \"x\"\n",
        )
        .unwrap();
        std::fs::write(root.path().join("crates/x/tests/there.rs"), "").unwrap();
        let verify = vec![
            "npm run nope".to_string(),
            "npm run build".to_string(),
            "cargo test -p x --test gone".to_string(),
            "cargo test -p x --test there".to_string(),
            "cargo test -p y".to_string(),
            "node --test tests/absent.test.cjs".to_string(),
        ];
        let found = code_checks(&view("", &[], &verify, root.path()), &|_| false);
        let missing: Vec<&str> = found
            .iter()
            .filter(|f| f.kind == "verify-missing")
            .map(|f| f.text.as_str())
            .collect();
        assert_eq!(missing.len(), 4, "{missing:?}");
        assert!(missing.iter().any(|t| t.contains("script `nope`")));
        assert!(missing.iter().any(|t| t.contains("target `gone`")));
        assert!(missing.iter().any(|t| t.contains("package `y`")));
        assert!(missing.iter().any(|t| t.contains("tests/absent.test.cjs")));
    }

    #[test]
    fn plan_gate_lets_a_listed_path_be_created() {
        let root = tempfile::tempdir().unwrap();
        let verify = vec!["node --test tests/new.test.cjs".to_string()];
        let paths = vec!["tests/new.test.cjs".to_string()];
        let mut v = view("", &[], &verify, root.path());
        v.paths = &paths;
        assert!(code_checks(&v, &|_| false).is_empty());
    }

    #[test]
    fn plan_gate_flags_a_whole_repo_command_in_verify() {
        let root = tempfile::tempdir().unwrap();
        let verify = vec![
            "npm run ci".to_string(),
            "cargo test -p sushiai-orch --lib plan_gate".to_string(),
            "cargo test --workspace".to_string(),
        ];
        let found = code_checks(&view("", &[], &verify, root.path()), &|_| false);
        let cost: Vec<&PlanFinding> = found.iter().filter(|f| f.kind == "verify-cost").collect();
        assert_eq!(cost.len(), 2, "{found:?}");
        assert!(cost.iter().all(|f| f.severity == Severity::Medium));
    }

    #[test]
    fn plan_gate_moves_whole_repo_commands_to_final_verify_when_another_remains() {
        use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
        let (app, dir) = test_app();
        let mut settings = app.settings.read().unwrap().clone();
        settings.review.clear();
        let mut task = task_with_status(TaskStatus::Drafting);
        let mut attempt = attempt_with_failure(1, "drafting");
        attempt.stage = Stage::Plan;
        attempt.status = AttemptStatus::Running;
        attempt.failure = None;
        task.attempts.push(attempt);
        let draft = brief::parse_plan(
            "```sushi-plan\n{\"title\":\"Run checks\",\"verify\":[\"npm run ci\",\"cargo test -p x --lib\"],\"finalVerify\":[\"npm run ci\"]}\n```",
        ).unwrap();
        store_draft(&app, &mut task, 0, &draft, &settings, dir.path());
        assert_eq!(task.verify, vec!["cargo test -p x --lib"]);
        assert_eq!(task.final_verify, vec!["npm run ci"]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("npm run ci") && d.contains("moved to finalVerify")));

        // A sole suite also runs once as the final gate, with review off.
        let sole = brief::parse_plan(
            "```sushi-plan\n{\"title\":\"Run checks\",\"verify\":[\"npm run ci\"]}\n```",
        )
        .unwrap();
        store_draft(&app, &mut task, 0, &sole, &settings, dir.path());
        assert!(task.verify.is_empty());
        assert_eq!(task.final_verify, vec!["npm run ci"]);
    }

    #[test]
    fn plan_gate_flags_a_dropped_shared_check_named_in_the_request() {
        let root = tempfile::tempdir().unwrap();
        let request = "Make it so, then run `npm run ci` and `echo hi`.";
        let shared = |c: &str| c == "npm run ci";
        let verify = vec!["cargo test -p x --lib".to_string()];
        let found = code_checks(&view(request, &[], &verify, root.path()), &shared);
        let dropped: Vec<&PlanFinding> =
            found.iter().filter(|f| f.kind == "shared-check").collect();
        assert_eq!(dropped.len(), 1, "{found:?}");
        assert_eq!(dropped[0].severity, Severity::Critical);
        let mut kept = view(request, &[], &[], root.path());
        let final_verify = vec!["npm run ci".to_string()];
        kept.final_verify = &final_verify;
        assert!(!kinds(&code_checks(&kept, &shared)).contains(&"shared-check"));
    }

    #[test]
    fn the_parent_plan_review_sees_the_complete_drafts_its_children_will_run() {
        let task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
        let mut child = part("child", &[], &["child.txt"], "test -f child.txt");
        child.title = "Write child".into();
        child.goal = "Keep the requested behaviour".into();
        child.criteria = vec!["Child behaves correctly".into()];
        child.verify = vec!["test -f child.txt".into()];
        child.final_verify = vec!["cargo test -p child".into()];
        child.tests = vec![PlannedTest {
            path: "child.test.ts".into(),
            content: "assert(child())".into(),
        }];
        child.screenshot = Some("npm run screenshot:child".into());
        child.held_out = Some(Check {
            criterion: 0,
            run: "secret-hidden-check".into(),
            baseline: None,
        });
        let prompt = build_review_prompt(&task, "Build the child", &[child]);
        let json: serde_json::Value =
            serde_json::from_str(prompt.strip_prefix(REVIEW_PROMPT).unwrap().trim()).unwrap();
        let child = &json["subtasks"][0];
        assert!(!prompt.contains("secret-hidden-check"));
        assert_eq!(child["heldOutCriterion"], 0);
        assert_eq!(child["goal"], "Keep the requested behaviour");
        assert_eq!(child["criteria"], json!(["Child behaves correctly"]));
        assert_eq!(child["verify"], json!(["test -f child.txt"]));
        assert_eq!(child["finalVerify"], json!(["cargo test -p child"]));
        assert_eq!(child["tests"][0]["content"], "assert(child())");
        assert_eq!(child["screenshot"], "npm run screenshot:child");
    }

    #[test]
    fn plan_gate_flags_overlapping_subtask_paths() {
        let root = tempfile::tempdir().unwrap();
        let subtasks = vec![
            part("a", &[], &["src/a"], "true"),
            part("b", &[], &["src/a/b.rs"], "true"),
            part("c", &[], &["docs"], "true"),
        ];
        let mut v = view("", &[], &[], root.path());
        v.subtasks = &subtasks;
        let found = code_checks(&v, &|_| false);
        let overlap: Vec<&PlanFinding> = found.iter().filter(|f| f.kind == "overlap").collect();
        assert_eq!(overlap.len(), 1, "{found:?}");
        assert_eq!(overlap[0].severity, Severity::Low);
    }

    #[test]
    fn a_review_reply_is_read_leniently() {
        let text = "Here:\n```json\n{\"findings\":[{\"severity\":\"critical\",\"kind\":\"Wrong split\",\"text\":\" a  b \"},{\"severity\":\"weird\",\"text\":\"c\"},{\"severity\":\"LOW\"}]}\n```";
        let found = parse_review_findings(text).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].severity, Severity::Critical);
        assert_eq!(found[0].kind, "wrong split");
        assert_eq!(found[0].text, "a b");
        assert_eq!(found[1].severity, Severity::Medium);
        assert_eq!(found[1].kind, "review");
        assert!(parse_review_findings("no json").is_err());
        assert!(parse_review_findings("{\"findings\": 3}").is_err());
        assert!(parse_review_findings("{\"findings\": []}")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn plan_gate_merges_overlapping_and_checkless_parts_and_keeps_the_rest() {
        let parts = vec![
            part("a", &[], &["src"], "check a"),
            part("b", &[], &["src/b.rs"], "check b"),
            part("c", &[], &["docs"], "check c"),
            part("d", &[], &[], "check d"),
            part("e", &["c"], &["web"], ""),
        ];
        let plan = merge_parts(&parts);
        assert_eq!(plan.separate, vec![2]);
        assert_eq!(plan.merged, vec![0, 1, 3, 4]);
    }

    #[test]
    fn complete_overlapping_drafts_keep_children_but_incomplete_overlaps_merge() {
        let mut a = part("a", &[], &["src/shared"], "");
        a.goal = "Add A".to_string();
        a.criteria = vec!["A is present".to_string()];
        a.verify = vec!["test -f src/shared/a".to_string()];
        let mut b = part("b", &[], &["src/shared"], "");
        b.goal = "Add B".to_string();
        b.criteria = vec!["B is present".to_string()];
        b.verify = vec!["test -f src/shared/b".to_string()];
        assert_eq!(
            merge_parts(&[a.clone(), b.clone()]),
            MergePlan {
                separate: vec![0, 1],
                merged: vec![],
            }
        );
        b.goal.clear();
        assert_eq!(
            merge_parts(&[a, b]),
            MergePlan {
                separate: vec![],
                merged: vec![0, 1],
            }
        );
    }

    #[test]
    fn plan_gate_orders_the_group_by_dependencies_and_keeps_planner_order_otherwise() {
        let parts = vec![
            part("a", &["c"], &["src"], "x"),
            part("b", &[], &["src"], "x"),
            part("c", &[], &["src"], "x"),
        ];
        assert_eq!(merge_parts(&parts).merged, vec![1, 2, 0]);
    }

    #[test]
    fn plan_gate_merges_everything_when_the_group_would_make_a_cycle() {
        // b is separate and depends on a (grouped); a depends on b through c.
        let parts = vec![
            part("a", &["b"], &[], "x"),
            part("b", &["a"], &["docs"], "x"),
        ];
        // a and b depend on each other: not a valid graph, but the group
        // mapping alone must not loop.
        let plan = merge_parts(&parts);
        assert!(plan.separate.is_empty());
        assert_eq!(plan.merged.len(), 2);

        let parts = vec![
            part("a", &[], &[], "x"),
            part("b", &["a"], &["docs"], "x"),
            part("c", &["b"], &[], "x"),
            part("d", &["b"], &["web"], "x"),
        ];
        // Group {a, c} depends on b (via c) and b depends on the group (via a).
        let plan = merge_parts(&parts);
        assert!(plan.separate.is_empty());
        assert_eq!(plan.merged, vec![0, 1, 2, 3]);
    }

    #[test]
    fn plan_gate_removes_a_criterion_and_reindexes_its_checks() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
        task.criteria = vec!["r".into(), "keep one".into(), "keep two".into()];
        task.visual_criteria = vec!["r".into(), "keep two".into()];
        task.checks = vec![
            Check {
                criterion: 0,
                run: "a".into(),
                baseline: None,
            },
            Check {
                criterion: 2,
                run: "c".into(),
                baseline: None,
            },
        ];
        remove_criteria(&mut task, &[0]);
        assert_eq!(task.criteria, vec!["keep one", "keep two"]);
        assert_eq!(task.visual_criteria, vec!["keep two"]);
        assert_eq!(task.checks.len(), 1);
        assert_eq!(task.checks[0].criterion, 1);
    }
}
