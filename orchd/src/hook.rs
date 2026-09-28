//! The Stop hook's decision function (spec "Stop hook (`hook.stop`)"), the
//! skills hook's pure parts (`variant.lean_context`), and the rtk hook's
//! pure parts (`variant.lean_output`): what text a hook payload contributes,
//! the `.agents/skills` catalog, which skills Jev's answers select, and each
//! hook's output. Kept as pure functions over facts so they can be unit
//! tested without a socket, a git repo, a classifier call or a real `rtk`
//! binary; `main.rs`/`engine.rs` do the IO and feed them in.

use crate::classify::{Answers, QuestionSpec};
use crate::harness::DENIED_BASH_COMMANDS;
use crate::model::VerifyOutcome;
use std::collections::HashSet;
use std::path::Path;

/// jev-belay-style probabilities the classifier returns for the
/// no-verify-commands path (spec step 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClassifiedOutcome {
    Complete,
    Partial,
    Blocked,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct JevBelayAnswers {
    pub claims_done: f64,
    pub claims_verified: f64,
    pub verification_applies: f64,
    pub outcome: ClassifiedOutcome,
}

pub struct StopFacts<'a> {
    pub blocks_so_far: u32,
    pub has_changed_files: bool,
    pub verify_configured: bool,
    pub verify_results: &'a [VerifyOutcome],
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopDecision {
    Allow,
    Block { reason: String },
}

const MAX_BLOCKS: u32 = 3;
const TAIL_CHARS: usize = 2000;

/// The *last* `max` chars -- the failure detail is usually at the end of a
/// verify command's output, not the start.
fn tail_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

/// Pure decision for `hook.stop`, matching spec steps 1-5 in order.
pub fn decide_stop(facts: &StopFacts, classifier: Option<&JevBelayAnswers>) -> StopDecision {
    if facts.blocks_so_far >= MAX_BLOCKS {
        return StopDecision::Allow;
    }
    if !facts.has_changed_files {
        return StopDecision::Allow;
    }
    if facts.verify_configured {
        if let Some(failed) = facts.verify_results.iter().find(|v| v.code != Some(0)) {
            let code = failed
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string());
            let tail = tail_chars(&failed.tail, TAIL_CHARS);
            return StopDecision::Block {
                reason: format!(
                    "Verification failed: {} exited {}.\n{}\nFix it, run the command again, then finish.",
                    failed.command, code, tail
                ),
            };
        }
        return StopDecision::Allow;
    }

    if let Some(a) = classifier {
        let should_block = a.claims_verified < 0.5
            && a.claims_done >= 0.7
            && a.verification_applies >= 0.5
            && a.outcome != ClassifiedOutcome::Blocked;
        if should_block {
            return StopDecision::Block {
                reason: "You said the work is done, but nothing verified it. Run the relevant checks, then finish.".to_string(),
            };
        }
    }

    StopDecision::Allow
}

// -- skills hook (`variant.lean_context`) --------------------------------

/// A PostToolUse whose tool printed less than this says nothing about the
/// skill the agent needs; the hook answers `{}` without asking the daemon.
const MIN_TOOL_OUTPUT_CHARS: usize = 200;
/// The agent's recent text sent to the classifier: its last this many chars.
pub const MAX_SKILLS_TEXT_CHARS: usize = 8_000;
const SKILL_MIN_P: f64 = 0.6;
const MAX_SKILLS_PER_CALL: usize = 3;
const MAX_SKILL_CHARS_PER_CALL: usize = 24_000;

/// What one skills-hook call asks the daemon about.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillsQuery {
    pub event: String,
    pub text: String,
}

/// The hook payload's event and the agent's recent text: the prompt for
/// UserPromptSubmit, the tool's input and output for PostToolUse. `None`
/// (answer `{}` without a daemon call) for any other event, or a tool
/// output too short to say anything.
pub fn skills_query(payload: &serde_json::Value) -> Option<SkillsQuery> {
    let event = payload.get("hook_event_name")?.as_str()?;
    let text = match event {
        "UserPromptSubmit" => payload.get("prompt")?.as_str()?.to_string(),
        "PostToolUse" => {
            let output = leaf_text(payload.get("tool_response")?);
            if output.chars().count() < MIN_TOOL_OUTPUT_CHARS {
                return None;
            }
            let input = payload.get("tool_input").map(leaf_text).unwrap_or_default();
            format!("{input}\n{output}")
        }
        _ => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    // A prompt is the brief: its head carries the goal. A tool's output
    // carries its news at the end.
    let text = if event == "UserPromptSubmit" {
        text.chars().take(MAX_SKILLS_TEXT_CHARS).collect()
    } else {
        tail_chars(&text, MAX_SKILLS_TEXT_CHARS)
    };
    Some(SkillsQuery {
        event: event.to_string(),
        text,
    })
}

/// Every string in a tool's input or response, one per line: a Bash
/// response is `{stdout, stderr, ...}`, a Read one nests the file content.
fn leaf_text(v: &serde_json::Value) -> String {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) if !s.is_empty() => out.push(s.clone()),
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => map.values().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out.join("\n")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// A `SKILL.md`: frontmatter between `---` lines, then the body. `None` for
/// bad frontmatter, a missing description, or `disable-model-invocation:
/// true` (those skills push, so only the owner starts them). The name
/// defaults to the skill's directory.
pub fn parse_skill(text: &str, dir_name: &str) -> Option<Skill> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut consumed = text.find('\n')? + 1;
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        let trimmed = line.trim_end();
        if trimmed == "---" {
            closed = true;
            break;
        }
        if trimmed.trim().is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // A folded continuation of the previous value.
            let (_, value) = fields.last_mut()?;
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(trimmed.trim());
            continue;
        }
        let (key, value) = trimmed.split_once(':')?;
        let value = value.trim();
        let value = if matches!(value, ">" | "|" | ">-" | "|-") {
            ""
        } else {
            value
        };
        fields.push((key.trim().to_string(), unquote(value)));
    }
    if !closed {
        return None;
    }
    let field = |k: &str| {
        fields
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.trim().to_string())
    };
    if field("disable-model-invocation").as_deref() == Some("true") {
        return None;
    }
    let description = field("description").filter(|d| !d.is_empty())?;
    let name = field("name")
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| dir_name.to_string());
    Some(Skill {
        name,
        description,
        body: text[consumed..].trim().to_string(),
    })
}

fn unquote(value: &str) -> String {
    let quoted = |q: char| value.len() >= 2 && value.starts_with(q) && value.ends_with(q);
    if quoted('"') {
        value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else if quoted('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_string()
    }
}

/// The worktree's `.agents/skills/*/SKILL.md`, by name; unreadable or
/// unparseable files are skipped, a missing directory is an empty catalog.
pub fn load_skill_catalog(worktree: &Path) -> std::io::Result<Vec<Skill>> {
    let dir = worktree.join(".agents").join("skills");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut skills: Vec<Skill> = Vec::new();
    for entry in entries.flatten() {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let Some(text) = read_skill_file(&entry.path().join("SKILL.md")) else {
            continue;
        };
        if let Some(skill) = parse_skill(&text, &dir_name) {
            skills.push(skill);
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills.dedup_by(|a, b| a.name == b.name);
    Ok(skills)
}

/// A skill file bigger than this is skipped rather than read: the catalog is
/// read on every hook call, and the file comes from the task's worktree.
const MAX_SKILL_FILE_BYTES: u64 = 64 * 1024;

/// A regular file's text, at most `MAX_SKILL_FILE_BYTES`; `None` for a
/// FIFO, a device, an oversized or a non-UTF-8 file.
fn read_skill_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SKILL_FILE_BYTES {
        return None;
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_SKILL_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_SKILL_FILE_BYTES {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// One Noul question per skill, named after it. The framing lets Jev turn
/// down skills that run the lead's loop (commit, delegate, open PRs)
/// without orchd naming any repo's skills.
pub fn skill_questions(skills: &[Skill]) -> Vec<QuestionSpec> {
    skills
        .iter()
        .map(|s| QuestionSpec::Noul {
            name: s.name.clone(),
            prompt: format!(
                "The agent implements one bounded coding task in its own worktree. It does not commit, push, open pull requests, delegate to other agents or run a multi-step task loop: an orchestrator does all of that. Would reading the skill \"{}\" help this agent with what the state shows it doing now? The skill: {}",
                s.name, s.description
            ),
        })
        .collect()
}

/// The skills to inject: p >= 0.6, highest first, at most 3, bodies within
/// 24,000 chars (a body that does not fit is skipped, never cut), none in
/// `seen`. `answers` is `None` when the classifier call failed: nothing.
pub fn select_skills<'a>(
    candidates: &'a [Skill],
    answers: Option<&Answers>,
    seen: &HashSet<String>,
) -> Vec<&'a Skill> {
    let Some(answers) = answers else {
        return vec![];
    };
    let mut scored: Vec<(f64, &Skill)> = candidates
        .iter()
        .filter(|s| !seen.contains(&s.name))
        .filter_map(|s| Some((answers.get(&s.name)?.noul?, s)))
        .filter(|(p, _)| *p >= SKILL_MIN_P)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    let mut picked = Vec::new();
    let mut chars = 0;
    for (_, skill) in scored {
        if picked.len() == MAX_SKILLS_PER_CALL {
            break;
        }
        let len = skill.body.chars().count();
        if chars + len > MAX_SKILL_CHARS_PER_CALL {
            continue;
        }
        chars += len;
        picked.push(skill);
    }
    picked
}

fn skill_bodies(picked: &[&Skill], heading: &str) -> String {
    picked
        .iter()
        .map(|s| format!("{heading} Skill: {}\n\n{}\n", s.name, s.body))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The skills hook's reply: the picked bodies as additional context, or
/// `{}` when nothing was picked.
pub fn skills_hook_output(event: &str, picked: &[&Skill]) -> serde_json::Value {
    if picked.is_empty() {
        return serde_json::json!({});
    }
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": skill_bodies(picked, "##"),
        }
    })
}

/// A fresh Codex brief's `## Skills for this task` section; empty when
/// nothing was picked.
pub fn skills_brief_section(picked: &[&Skill]) -> String {
    if picked.is_empty() {
        return String::new();
    }
    format!(
        "## Skills for this task\n\n{}\n",
        skill_bodies(picked, "###")
    )
}

// -- rtk hook (`variant.lean_output`) ------------------------------------

/// A PreToolUse payload's `tool_input.command`, the command `orchd hook rtk`
/// asks `rtk rewrite` about. `None` for anything else the hook must fail
/// open on: stdin that isn't JSON, a payload with no `tool_input.command`,
/// or a `command` that isn't a string.
pub fn rtk_hook_command(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("tool_input")?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

/// Whether `command` touches a prefix `orchd hook rtk` must never let past
/// as a rewrite -- checked against both the original and any candidate
/// rewrite, since a rewrite that only introduces the denied prefix (e.g.
/// `rtk git commit ...`) is exactly what would otherwise launder past
/// `Bash(git commit:*)`/`Bash(git push:*)` through the hook's own
/// `permissionDecision: allow`.
pub fn touches_denied_bash_command(command: &str) -> bool {
    DENIED_BASH_COMMANDS.iter().any(|p| command.contains(p))
}

/// `orchd hook rtk`'s reply once `rtk rewrite <original_command>` has run
/// (or failed to). `Some(hookSpecificOutput)` only when it exited 0 with a
/// different, non-empty command that touches no denied git prefix either
/// way; `None` (caller prints `{}`) for everything else -- a non-zero exit
/// (rtk's common "nothing to rewrite" answer), a timeout, rtk missing from
/// PATH, the same command echoed back, or a denied rewrite.
/// rtk subcommands that only condense a build/test tool's output and keep
/// its exit code. `rtk read`/`grep`/`ls` reshape content itself (`rtk read`
/// drops comment lines, and `tail -n` becomes a different `-n`), so a
/// rewrite into them would show the agent code that is not on disk.
const RTK_CONDENSING: [&str; 6] = ["cargo", "npm", "pnpm", "tsc", "pytest", "go"];

fn only_condensing_rtk(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words
        .windows(2)
        .all(|w| w[0] != "rtk" || RTK_CONDENSING.contains(&w[1]))
        && words.last() != Some(&"rtk")
}

/// `rtk rewrite`'s exit code: 0 rewrites and allows, 3 rewrites but leaves
/// the permission decision to Claude Code's own rules (rtk >= 0.50 checks
/// them), 1 has no rewrite and 2 is a deny rule; anything else declines.
pub fn rtk_rewrite_output(
    tool_input: &serde_json::Value,
    original_command: &str,
    rtk_exit: Option<i32>,
    rtk_stdout: &str,
) -> Option<serde_json::Value> {
    let allow = match rtk_exit {
        Some(0) => true,
        Some(3) => false,
        _ => return None,
    };
    let rewritten = rtk_stdout.trim();
    if rewritten.is_empty() || rewritten == original_command.trim() {
        return None;
    }
    if touches_denied_bash_command(original_command) || touches_denied_bash_command(rewritten) {
        return None;
    }
    if original_command.contains('>') || !only_condensing_rtk(rewritten) {
        return None;
    }
    let mut updated_input = tool_input.clone();
    updated_input["command"] = serde_json::Value::String(rewritten.to_string());
    let mut out = serde_json::json!({
        "hookEventName": "PreToolUse",
        "updatedInput": updated_input,
    });
    if allow {
        out["permissionDecision"] = "allow".into();
    }
    Some(serde_json::json!({ "hookSpecificOutput": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(
        blocks: u32,
        changed: bool,
        verify_configured: bool,
        verify: &'a [VerifyOutcome],
    ) -> StopFacts<'a> {
        StopFacts {
            blocks_so_far: blocks,
            has_changed_files: changed,
            verify_configured,
            verify_results: verify,
        }
    }

    #[test]
    fn allows_once_three_blocks_reached_for_this_attempt() {
        let d = decide_stop(&facts(3, true, true, &[]), None);
        assert_eq!(d, StopDecision::Allow);
        let d = decide_stop(&facts(4, true, true, &[]), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn allows_when_no_changed_files() {
        let d = decide_stop(&facts(0, false, true, &[]), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn blocks_with_failing_verify_tail() {
        let verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "FAIL src/x.test.ts".into(),
            ms: 100,
        }];
        let d = decide_stop(&facts(0, true, true, &verify), None);
        match d {
            StopDecision::Block { reason } => {
                assert!(reason.contains("npm test exited 1"));
                assert!(reason.contains("FAIL src/x.test.ts"));
            }
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn allows_when_verify_passes() {
        let verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(0),
            tail: "ok".into(),
            ms: 100,
        }];
        let d = decide_stop(&facts(0, true, true, &verify), None);
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn verify_tail_keeps_the_last_2000_chars_not_the_first() {
        // The interesting part of a failing command's output (the actual
        // assertion/error) is almost always at the end, not the start.
        let long_tail = format!("{}END_MARKER", "x".repeat(5000));
        let verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: long_tail,
            ms: 100,
        }];
        let d = decide_stop(&facts(0, true, true, &verify), None);
        match d {
            StopDecision::Block { reason } => {
                assert!(reason.len() < 5100);
                assert!(
                    reason.contains("END_MARKER"),
                    "should keep the tail end of the output, not the head: {reason}"
                );
            }
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn no_verify_commands_blocks_on_jev_belay_thresholds() {
        let a = JevBelayAnswers {
            claims_done: 0.9,
            claims_verified: 0.2,
            verification_applies: 0.8,
            outcome: ClassifiedOutcome::Complete,
        };
        let d = decide_stop(&facts(0, true, false, &[]), Some(&a));
        match d {
            StopDecision::Block { reason } => assert!(reason.contains("nothing verified it")),
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn no_verify_commands_allows_when_outcome_is_blocked() {
        let a = JevBelayAnswers {
            claims_done: 0.9,
            claims_verified: 0.2,
            verification_applies: 0.8,
            outcome: ClassifiedOutcome::Blocked,
        };
        let d = decide_stop(&facts(0, true, false, &[]), Some(&a));
        assert_eq!(d, StopDecision::Allow);
    }

    #[test]
    fn no_verify_commands_and_no_classifier_allows() {
        let d = decide_stop(&facts(0, true, false, &[]), None);
        assert_eq!(d, StopDecision::Allow);
    }

    // -- skills hook --

    #[test]
    fn prompt_query_keeps_the_head_tool_query_the_tail() {
        let brief = format!("GOAL{}", "x".repeat(MAX_SKILLS_TEXT_CHARS));
        let q = skills_query(&serde_json::json!({
            "hook_event_name": "UserPromptSubmit", "prompt": brief
        }))
        .unwrap();
        assert!(q.text.starts_with("GOAL"));
        let out = format!("{}END", "y".repeat(MAX_SKILLS_TEXT_CHARS));
        let q = skills_query(&serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_response": {"stdout": out}
        }))
        .unwrap();
        assert!(q.text.ends_with("END"));
    }

    #[test]
    fn oversized_or_non_regular_skill_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!("orchd-skillfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.md");
        std::fs::write(&big, vec![b'a'; MAX_SKILL_FILE_BYTES as usize + 1]).unwrap();
        let ok = dir.join("ok.md");
        std::fs::write(&ok, "fine").unwrap();
        assert_eq!(read_skill_file(&big), None);
        assert_eq!(read_skill_file(&dir), None);
        assert_eq!(read_skill_file(&ok).as_deref(), Some("fine"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use crate::classify::Answer;

    fn skill(name: &str, body_len: usize) -> Skill {
        Skill {
            name: name.into(),
            description: format!("{name} description"),
            body: "b".repeat(body_len),
        }
    }

    fn answers(ps: &[(&str, f64)]) -> Answers {
        ps.iter()
            .map(|(n, p)| {
                (
                    n.to_string(),
                    Answer {
                        noul: Some(*p),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn names(picked: &[&Skill]) -> Vec<String> {
        picked.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn user_prompt_submit_asks_about_the_prompt() {
        let q = skills_query(&serde_json::json!({
            "hook_event_name": "UserPromptSubmit", "prompt": "Fix the manifest validator"
        }))
        .unwrap();
        assert_eq!(q.event, "UserPromptSubmit");
        assert_eq!(q.text, "Fix the manifest validator");
    }

    #[test]
    fn a_short_tool_output_asks_nothing() {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": "x".repeat(500)},
            "tool_response": {"stdout": "ok", "stderr": "", "interrupted": false}
        });
        assert_eq!(skills_query(&payload), None);
    }

    #[test]
    fn a_tool_call_sends_its_input_and_output_capped_to_the_last_chars() {
        let output = format!("{}END", "o".repeat(20_000));
        let q = skills_query(&serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Read",
            "tool_input": {"file_path": "/w/src/a.ts"},
            "tool_response": {"file": {"content": output}}
        }))
        .unwrap();
        assert_eq!(q.event, "PostToolUse");
        assert_eq!(q.text.chars().count(), MAX_SKILLS_TEXT_CHARS);
        assert!(q.text.ends_with("END"));

        let short = skills_query(&serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_input": {"command": "cat log"},
            "tool_response": {"stdout": "l".repeat(250)}
        }))
        .unwrap();
        assert!(short.text.starts_with("cat log\n"), "{}", short.text);
    }

    #[test]
    fn other_events_and_bad_payloads_ask_nothing() {
        assert_eq!(
            skills_query(&serde_json::json!({"hook_event_name": "Stop"})),
            None
        );
        assert_eq!(skills_query(&serde_json::Value::Null), None);
        assert_eq!(
            skills_query(&serde_json::json!({"hook_event_name": "UserPromptSubmit"})),
            None
        );
    }

    #[test]
    fn parses_frontmatter_and_the_body_after_it() {
        let s = parse_skill(
            "---\nname: deslop\ndescription: \"Diff-scoped cleanup: strip \\\"slop\\\".\"\n---\n\n# Deslop\n\nDo it.\n",
            "dir",
        )
        .unwrap();
        assert_eq!(s.name, "deslop");
        assert_eq!(s.description, "Diff-scoped cleanup: strip \"slop\".");
        assert_eq!(s.body, "# Deslop\n\nDo it.");

        let folded = parse_skill(
            "---\ndescription: >\n  One\n  two.\ncontext: fork\n---\nbody\n",
            "from-dir",
        )
        .unwrap();
        assert_eq!(folded.name, "from-dir");
        assert_eq!(folded.description, "One two.");
    }

    #[test]
    fn skips_a_skill_without_a_description() {
        assert_eq!(parse_skill("---\nname: x\n---\nbody\n", "x"), None);
        assert_eq!(
            parse_skill("---\nname: x\ndescription:\n---\nbody\n", "x"),
            None
        );
    }

    #[test]
    fn skips_bad_frontmatter() {
        assert_eq!(parse_skill("# no frontmatter\n", "x"), None);
        assert_eq!(
            parse_skill("---\nname: x\ndescription: y\nbody never closes\n", "x"),
            None
        );
        assert_eq!(
            parse_skill("---\nname: x\ndescription: y\n", "x"),
            None,
            "unclosed"
        );
    }

    #[test]
    fn skips_skills_only_the_owner_starts() {
        let text = "---\nname: ship-pr\ndescription: Opens a PR.\ndisable-model-invocation: true\n---\nbody\n";
        assert_eq!(parse_skill(text, "ship-pr"), None);
        let allowed = text.replace("true", "false");
        assert!(parse_skill(&allowed, "ship-pr").is_some());
    }

    #[test]
    fn loads_the_worktree_catalog_by_name() {
        let wt = tempfile::tempdir().unwrap();
        assert_eq!(load_skill_catalog(wt.path()).unwrap(), vec![]);
        let dir = wt.path().join(".agents/skills");
        for (name, text) in [
            (
                "b-skill",
                "---\nname: b-skill\ndescription: B.\n---\nB body\n",
            ),
            (
                "a-skill",
                "---\nname: a-skill\ndescription: A.\n---\nA body\n",
            ),
            ("broken", "no frontmatter"),
            (
                "pusher",
                "---\nname: pusher\ndescription: P.\ndisable-model-invocation: true\n---\nP\n",
            ),
        ] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("SKILL.md"), text).unwrap();
        }
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        let catalog = load_skill_catalog(wt.path()).unwrap();
        let names: Vec<&str> = catalog.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a-skill", "b-skill"]);
        assert_eq!(catalog[0].body, "A body");
    }

    #[test]
    fn asks_one_noul_question_per_skill_framed_for_a_bounded_implementer() {
        let qs = skill_questions(&[skill("a", 1), skill("b", 1)]);
        assert_eq!(qs.len(), 2);
        match &qs[1] {
            QuestionSpec::Noul { name, prompt } => {
                assert_eq!(name, "b");
                assert!(prompt.contains("b description"));
                assert!(prompt.contains("does not commit") && prompt.contains("delegate"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn selects_only_skills_at_or_above_the_threshold() {
        let skills = [skill("low", 10), skill("edge", 10), skill("high", 10)];
        let a = answers(&[("low", 0.59), ("edge", 0.6), ("high", 0.9)]);
        let picked = select_skills(&skills, Some(&a), &HashSet::new());
        assert_eq!(names(&picked), ["high", "edge"]);
    }

    #[test]
    fn selects_the_highest_three_first() {
        let skills = [skill("a", 1), skill("b", 1), skill("c", 1), skill("d", 1)];
        let a = answers(&[("a", 0.7), ("b", 0.95), ("c", 0.8), ("d", 0.9)]);
        let picked = select_skills(&skills, Some(&a), &HashSet::new());
        assert_eq!(names(&picked), ["b", "d", "c"]);
    }

    #[test]
    fn never_selects_a_skill_already_injected_in_the_session() {
        let skills = [skill("a", 1), skill("b", 1)];
        let a = answers(&[("a", 0.9), ("b", 0.8)]);
        let seen: HashSet<String> = ["a".to_string()].into();
        assert_eq!(names(&select_skills(&skills, Some(&a), &seen)), ["b"]);
    }

    #[test]
    fn skips_a_body_that_does_not_fit_the_budget_instead_of_cutting_it() {
        let skills = [
            skill("big", 20_000),
            skill("huge", 5_000),
            skill("small", 3_000),
        ];
        let a = answers(&[("big", 0.9), ("huge", 0.8), ("small", 0.7)]);
        let picked = select_skills(&skills, Some(&a), &HashSet::new());
        assert_eq!(names(&picked), ["big", "small"]);
        assert_eq!(picked[0].body.len(), 20_000, "never truncated");
        let alone = [skill("too-big", 24_001)];
        let a = answers(&[("too-big", 0.99)]);
        assert!(select_skills(&alone, Some(&a), &HashSet::new()).is_empty());
    }

    #[test]
    fn a_classifier_error_selects_nothing() {
        let skills = [skill("a", 1)];
        assert!(select_skills(&skills, None, &HashSet::new()).is_empty());
        // An answer without a probability for the skill selects nothing too.
        assert!(select_skills(&skills, Some(&Answers::new()), &HashSet::new()).is_empty());
    }

    #[test]
    fn hook_output_carries_each_picked_body_under_its_name() {
        let a = Skill {
            name: "deslop".into(),
            description: String::new(),
            body: "Strip narration.".into(),
        };
        let b = Skill {
            name: "sushiai-testing".into(),
            description: String::new(),
            body: "Pick proof.".into(),
        };
        let out = skills_hook_output("PostToolUse", &[&a, &b]);
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            "## Skill: deslop\n\nStrip narration.\n\n## Skill: sushiai-testing\n\nPick proof.\n"
        );
        assert_eq!(
            skills_hook_output("UserPromptSubmit", &[]),
            serde_json::json!({})
        );

        let section = skills_brief_section(&[&a]);
        assert_eq!(
            section,
            "## Skills for this task\n\n### Skill: deslop\n\nStrip narration.\n\n"
        );
        assert_eq!(skills_brief_section(&[]), "");
    }

    #[test]
    fn no_verify_commands_allows_below_thresholds() {
        let a = JevBelayAnswers {
            claims_done: 0.5, // below 0.7 threshold
            claims_verified: 0.2,
            verification_applies: 0.8,
            outcome: ClassifiedOutcome::Complete,
        };
        let d = decide_stop(&facts(0, true, false, &[]), Some(&a));
        assert_eq!(d, StopDecision::Allow);
    }

    // -- rtk hook --

    #[test]
    fn rtk_hook_command_reads_tool_input_command_or_fails_open() {
        assert_eq!(
            rtk_hook_command(&serde_json::json!({"tool_input": {"command": "npm test"}})),
            Some("npm test".to_string())
        );
        assert_eq!(rtk_hook_command(&serde_json::Value::Null), None);
        assert_eq!(rtk_hook_command(&serde_json::json!({})), None);
        assert_eq!(
            rtk_hook_command(&serde_json::json!({"tool_input": {}})),
            None
        );
        assert_eq!(
            rtk_hook_command(&serde_json::json!({"tool_input": {"command": 1}})),
            None
        );
    }

    #[test]
    fn rtk_rewrite_output_allows_only_a_clean_different_rewrite() {
        let input =
            serde_json::json!({"command": "npm test", "description": "run tests", "timeout": 1000});
        let out = rtk_rewrite_output(&input, "npm test", Some(0), "rtk npm test\n").unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"],
            serde_json::json!({
                "command": "rtk npm test",
                "description": "run tests",
                "timeout": 1000
            })
        );

        // Same command back, non-zero exit, empty output: all decline.
        assert!(rtk_rewrite_output(&input, "npm test", Some(0), "npm test").is_none());
        assert!(rtk_rewrite_output(&input, "npm test", Some(1), "npm test --silent").is_none());
        assert!(rtk_rewrite_output(&input, "npm test", Some(0), "").is_none());
        assert!(rtk_rewrite_output(&input, "npm test", Some(0), "   ").is_none());
    }

    #[test]
    fn rtk_rewrite_output_leaves_an_ask_answer_to_claude_code() {
        let input = serde_json::json!({"command": "cargo test"});
        let out = rtk_rewrite_output(&input, "cargo test", Some(3), "rtk cargo test").unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk cargo test"
        );
        assert!(out["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
        assert!(rtk_rewrite_output(&input, "cargo test", Some(2), "rtk cargo test").is_none());
        assert!(rtk_rewrite_output(&input, "cargo test", None, "rtk cargo test").is_none());
    }

    #[test]
    fn rtk_rewrite_output_declines_content_changing_rewrites() {
        // Real rtk 0.28 rewrites that alter what the agent sees or writes.
        for (orig, rewritten) in [
            ("cat a.rs > b.rs", "rtk read a.rs > b.rs"),
            ("cat a.rs", "rtk read a.rs"),
            ("tail -n 50 log", "rtk read -n 50 log"),
            ("rg foo", "rtk grep foo"),
            ("ls -la", "rtk ls -la"),
            ("npx eslint .", "rtk lint ."),
            ("cargo test > out.txt", "rtk cargo test > out.txt"),
        ] {
            let input = serde_json::json!({ "command": orig });
            assert!(
                rtk_rewrite_output(&input, orig, Some(0), rewritten).is_none(),
                "{orig}"
            );
        }
        for (orig, rewritten) in [
            ("cargo test", "rtk cargo test"),
            (
                "cargo build && git status | head -5",
                "rtk cargo build && git status | head -5",
            ),
            ("tsc --noEmit", "rtk tsc --noEmit"),
        ] {
            let input = serde_json::json!({ "command": orig });
            assert!(
                rtk_rewrite_output(&input, orig, Some(0), rewritten).is_some(),
                "{orig}"
            );
        }
    }

    #[test]
    fn rtk_rewrite_output_never_rewrites_past_git_commit_or_push() {
        let input = serde_json::json!({"command": "git commit -m x"});
        assert!(
            rtk_rewrite_output(&input, "git commit -m x", Some(0), "git commit -am x").is_none()
        );
        let input = serde_json::json!({"command": "git push"});
        assert!(rtk_rewrite_output(&input, "git push", Some(0), "git push origin main").is_none());
        // Even an unrelated original must not be rewritten into one.
        let input = serde_json::json!({"command": "ls"});
        assert!(rtk_rewrite_output(&input, "ls", Some(0), "rtk git commit -am x").is_none());
        assert!(touches_denied_bash_command("git commit -m x"));
        assert!(touches_denied_bash_command("git push origin main"));
        assert!(!touches_denied_bash_command("git status"));
    }
}
