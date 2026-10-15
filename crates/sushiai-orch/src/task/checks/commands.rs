use crate::task::state::TaskData;

/// Image extensions an evidence screenshot may have, with their MIME types.
pub const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("svg", "image/svg+xml"),
    ("bmp", "image/bmp"),
    ("avif", "image/avif"),
];

/// The MIME type of an image path, if its extension is an evidence image type.
pub fn image_mime(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// Words that make an `artifacts/` path in a check an image.
const IMAGE_WORDS: &[&str] = &[
    "image",
    "img",
    "picture",
    "photo",
    "snapshot",
    "capture",
    "visual",
    "thumbnail",
    "render",
];

/// The text after `-- check:`, lowercased with `-` and `_` made spaces.
fn normalized_check(text: &str) -> Option<String> {
    let (_, check) = text.split_once("-- check:")?;
    Some(check.to_ascii_lowercase().replace(['-', '_'], " "))
}

/// Whether a normalized check mentions a screenshot, an image word or an
/// image extension.
fn mentions_image(check: &str) -> bool {
    check.contains("screenshot")
        || check.contains("screen shot")
        || IMAGE_WORDS.iter().any(|word| check.contains(word))
        || IMAGE_TYPES.iter().any(|(ext, _)| check.contains(ext))
}

/// A criterion is visual when its `-- check:` names a screenshot or an image
/// under `artifacts/`.
pub fn is_visual_criterion(text: &str) -> bool {
    let Some(check) = normalized_check(text) else {
        return false;
    };
    check.contains("screenshot")
        || check.contains("screen shot")
        || (check.contains("artifacts/") && IMAGE_WORDS.iter().any(|word| check.contains(word))
            || IMAGE_TYPES.iter().any(|(ext, _)| check.contains(ext)))
}

/// A criterion whose `-- check:` runs a test or build command and mentions no
/// image: its proof is the command's output, never a picture.
pub fn is_command_check(text: &str) -> bool {
    let Some(check) = normalized_check(text) else {
        return false;
    };
    if mentions_image(&check) {
        return false;
    }
    let has_word = check
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| COMMAND_TOOLS.contains(&w));
    has_word
        || ["go test", "playwright test", "test runner"]
            .iter()
            .any(|p| check.contains(p))
}

/// Test and build tools whose name in a `-- check:` makes it a command check.
const COMMAND_TOOLS: &[&str] = &[
    "cargo", "npm", "npx", "pnpm", "yarn", "bun", "node", "deno", "pytest", "tox", "uv", "make",
    "cmake", "ctest", "jest", "vitest", "gradle", "gradlew", "mvn", "mvnw", "dotnet", "rspec",
    "rake", "phpunit",
];

/// Whether `command` runs as it is: its first word is a test or build tool.
fn starts_with_tool(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    COMMAND_TOOLS.contains(&first)
        || ["go test", "playwright test"]
            .iter()
            .any(|p| lower.starts_with(p))
}

/// The command a criterion's `-- check:` part names: the first backtick span
/// that starts with a test or build tool, else the whole part when it does.
/// A part led by prose (`run cargo test`) names none.
pub fn check_part_command(text: &str) -> Option<String> {
    let (_, part) = text.split_once("-- check:")?;
    let part = part.trim();
    if part.contains('`') {
        return part
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::trim)
            .find(|s| starts_with_tool(s))
            .map(str::to_string);
    }
    starts_with_tool(part).then(|| part.to_string())
}

impl TaskData {
    /// The criteria that need an image as evidence.
    pub fn visual_criteria_texts(&self) -> Vec<&str> {
        self.criteria
            .iter()
            .filter(|c| {
                !is_command_check(c) && (self.visual_criteria.contains(c) || is_visual_criterion(c))
            })
            .map(String::as_str)
            .collect()
    }
}

/// Leading tokens of a repo's own CI entry. A command segment that starts
/// with one of these (after `cd <dir>` and `VAR=x`) is a CI entry.
const CI_ENTRIES: [&[&str]; 11] = [
    &["npm", "run", "ci"],
    &["npm", "test"],
    &["npm", "run", "test"],
    &["pnpm", "test"],
    &["pnpm", "run", "ci"],
    &["pnpm", "ci"],
    &["yarn", "test"],
    &["yarn", "ci"],
    &["cargo", "test"],
    &["make", "test"],
    &["make", "ci"],
];

pub(crate) fn flat_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn is_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether one shell segment runs a CI entry.
fn segment_runs_ci(segment: &str) -> bool {
    let mut tokens: Vec<&str> = segment.split_whitespace().collect();
    loop {
        match tokens.as_slice() {
            ["cd", _, ..] | ["timeout", _, ..] => tokens.drain(..2),
            ["env", ..] => tokens.drain(..1),
            [first, ..] if is_assignment(first) => tokens.drain(..1),
            _ => break,
        };
    }
    CI_ENTRIES.iter().any(|entry| tokens.starts_with(entry))
}

/// The `&&`, `||`, `;`, `|` and newline separated parts of `command`.
pub(crate) fn command_segments(command: &str) -> Vec<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace([';', '|'], "\n")
        .replace(['(', ')'], " ")
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether any part of `command` runs a CI entry.
pub(crate) fn runs_ci_entry(command: &str) -> bool {
    command_segments(command)
        .iter()
        .any(|segment| segment_runs_ci(segment))
}

/// The path-like arguments of a check: a token that holds a `/`, a glob
/// character (`*`, `?`, `[`) or a file extension (`name.ext`). Quotes are
/// stripped; flags, URLs, `VAR=x` assignments, shell operators, absolute
/// paths and bare words (`npm`, `run`, `test`) are left out.
pub(crate) fn check_targets(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut redirected = false;
    for token in command.split_whitespace() {
        // The file a `>` writes is not something the check reads.
        if std::mem::replace(&mut redirected, matches!(token, ">" | ">>")) {
            continue;
        }
        let token = token
            .trim_matches(|c| c == '"' || c == '\'')
            .trim_end_matches([';', ',', ')'])
            .trim_start_matches('(')
            .trim_matches(|c| c == '"' || c == '\'');
        let token = path_without_location(token);
        if token.is_empty()
            || token.starts_with(['-', '/', '~', '$', '>', '<', '|', '&'])
            || token.contains("://")
            || token.contains('>')
            || token.contains('<')
            || is_assignment(token)
        {
            continue;
        }
        let has_ext = token.rsplit_once('.').is_some_and(|(name, ext)| {
            !name.is_empty()
                && !name.ends_with('/')
                && ext.len() <= 10
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        });
        if token.contains(['/', '*', '?', '[']) || has_ext {
            let token = token.strip_prefix("./").unwrap_or(token).to_string();
            if !token.is_empty() && !out.contains(&token) {
                out.push(token);
            }
        }
    }
    out
}

fn path_without_location(path: &str) -> &str {
    let mut parts = path.split(':');
    let first = parts.next().unwrap_or(path);
    let line = parts.next();
    if line.is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        && parts
            .next()
            .is_none_or(|n| n.chars().all(|c| c.is_ascii_digit()))
    {
        first
    } else {
        path
    }
}

/// `grep` reads a pattern that starts with `-` as an option, so such a check
/// cannot run. Returns `command` with `--` inserted before each such
/// pattern, or `None` when nothing needs it. Quote-aware, and it leaves
/// `-e`/`-f` forms alone (their next word is already an argument).
pub(crate) fn fix_grep_dashes(command: &str) -> Option<String> {
    // Words with their byte offsets; quotes keep a word whole.
    let mut words: Vec<(usize, String, bool)> = Vec::new();
    let mut cur: Option<(usize, String, bool)> = None;
    let mut quote: Option<char> = None;
    for (i, c) in command.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.get_or_insert((i, String::new(), false)).1.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.get_or_insert((i, String::new(), true)).2 = true;
            }
            (None, c) if c.is_whitespace() => words.extend(cur.take()),
            (None, c) => cur.get_or_insert((i, String::new(), false)).1.push(c),
        }
    }
    words.extend(cur);
    let mut inserts: Vec<usize> = Vec::new();
    let mut at_start = true;
    let mut i = 0;
    while i < words.len() {
        let w = words[i].1.as_str();
        if matches!(w, "&&" | "||" | ";" | "|" | "(") {
            at_start = true;
            i += 1;
            continue;
        }
        if !(at_start && matches!(w, "grep" | "egrep" | "fgrep")) {
            at_start = false;
            i += 1;
            continue;
        }
        at_start = false;
        i += 1;
        let mut takes_arg = false;
        while i < words.len() {
            let o = words[i].1.as_str();
            // `--long` is an option quoted or not; a quoted word with one
            // dash is the pattern.
            if words[i].2 && !o.starts_with("--") {
                break;
            }
            if o == "--" {
                i = words.len();
                takes_arg = true;
                break;
            }
            let short_cluster = o.starts_with('-') && !o.starts_with("--") && o.len() > 1;
            if short_cluster && o[1..].contains(['e', 'f']) {
                takes_arg = true;
                break;
            }
            if o.starts_with('-') && o.len() > 1 {
                i += 1;
                continue;
            }
            break;
        }
        if !takes_arg && i < words.len() && words[i].1.starts_with('-') && words[i].1.len() > 1 {
            inserts.push(words[i].0);
            i += 1;
        }
    }
    if inserts.is_empty() {
        return None;
    }
    let mut out = command.to_string();
    for at in inserts.into_iter().rev() {
        out.insert_str(at, "-- ");
    }
    Some(out)
}

pub fn held_out_label(criterion: usize) -> String {
    format!("held-out check (criterion {criterion})")
}
