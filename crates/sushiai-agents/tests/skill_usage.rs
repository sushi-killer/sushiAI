//! The transcript usage index: what counts as a use, and the incremental cache.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sushiai_agents::harness::Homes;
use sushiai_agents::skill_usage::{index, IndexStats, SkillUsage, Usage};
use sushiai_agents::skills::scan;

/// 2026-10-19T12:00:00Z
const NOW: i64 = 1_792_411_200_000;
const RECENT: &str = "2026-10-10T08:00:00.000Z";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn homes(&self) -> Homes {
        Homes::new(self.dir.path(), None, None)
    }
    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache").join("skill-usage.json")
    }
    fn claude(&self, name: &str) -> PathBuf {
        self.dir.path().join(".claude/projects/-proj").join(name)
    }
    fn codex(&self, name: &str) -> PathBuf {
        self.dir.path().join(".codex/sessions/2026/10").join(name)
    }
    fn index(&self) -> IndexStats {
        index(&self.homes(), &self.cache(), NOW).unwrap()
    }
    /// A skill folder of the harness (`.claude` or `.codex`) named `folder`, declaring `name`.
    fn skill(&self, tool: &str, folder: &str, name: &str) {
        let text = format!("---\nname: {name}\ndescription: d\n---\nbody\n");
        write_text(
            &self
                .dir
                .path()
                .join(tool)
                .join("skills")
                .join(folder)
                .join("SKILL.md"),
            &text,
        );
    }
    /// The usage of every row called `name` of `harness` (`Claude Code` or `Codex`).
    fn usage(&self, harness: &str, name: &str) -> SkillUsage {
        let usage = Usage::load(&self.cache());
        let rows = scan(&self.homes(), None);
        let row = rows
            .iter()
            .find(|r| r.name == name && serde_json::to_value(r.harness).unwrap() == harness)
            .unwrap();
        usage.of(row, NOW)
    }
}

fn write_text(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn write(path: &Path, lines: &[String]) {
    write_text(path, &(lines.join("\n") + "\n"));
}

fn append(path: &Path, text: &str) {
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn claude_tool(skill: &str, at: &str) -> String {
    json!({"type":"assistant","timestamp":at,"cwd":"/work/app","sessionId":"s1",
        "message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":skill}}]}})
    .to_string()
}

fn claude_slash(skill: &str, at: &str) -> String {
    json!({"type":"user","timestamp":at,"cwd":"/work/app","sessionId":"s1",
        "message":{"content":format!("<command-name>/{skill}</command-name>\n<command-args></command-args>")}})
    .to_string()
}

fn codex_call(kind: &str, field: &str, text: &str, at: &str) -> String {
    json!({"timestamp":at,"type":"response_item","payload":{"type":kind, field:text}}).to_string()
}

fn codex_meta() -> String {
    json!({"timestamp":RECENT,"type":"session_meta","payload":{"id":"c1","cwd":"/work/cx"}})
        .to_string()
}

#[test]
fn claude_skill_tool_and_slash_command_count() {
    let fx = Fixture::new();
    fx.skill(".claude", "vault", "vault");
    write(
        &fx.claude("a.jsonl"),
        &[
            claude_tool("vault", RECENT),
            claude_slash("vault", "2026-10-11T08:00:00Z"),
            claude_tool("other", RECENT),
        ],
    );
    assert_eq!(fx.index().events, 3);
    let u = fx.usage("Claude Code", "vault");
    assert_eq!(u.uses90d, 2);
    assert_eq!(u.last_used_at.as_deref(), Some("2026-10-11T08:00:00.000Z"));
    assert_eq!(u.events[0].project, "/work/app");
    assert_eq!(u.events[0].session, "s1");
    assert_eq!(u.indexed_at.as_deref(), Some("2026-10-19T12:00:00.000Z"));
}

#[test]
fn claude_subagent_files_count() {
    let fx = Fixture::new();
    write(
        &fx.claude("s1/subagents/agent-x.jsonl"),
        &[claude_tool("vault", RECENT)],
    );
    assert_eq!(fx.index().events, 1);
}

#[test]
fn claude_text_that_is_not_a_tool_use_does_not_count() {
    let fx = Fixture::new();
    let text_only = json!({"type":"assistant","timestamp":RECENT,"message":{"content":[
        {"type":"text","text":"I will use \"name\":\"Skill\" here"}]}});
    let attachment = json!({"type":"attachment","timestamp":RECENT,"attachment":{
        "tools":[{"name":"Skill","input":{"skill":"vault"}}]}});
    let other_tool = json!({"type":"assistant","timestamp":RECENT,"message":{"content":[
        {"type":"tool_use","name":"Bash","input":{"skill":"vault"}}]}});
    let pasted = json!({"type":"user","timestamp":RECENT,"message":{
        "content":"see <command-name>/vault</command-name> in the log"}});
    write(
        &fx.claude("a.jsonl"),
        &[
            text_only.to_string(),
            attachment.to_string(),
            other_tool.to_string(),
            pasted.to_string(),
        ],
    );
    assert_eq!(fx.index().events, 0);
}

#[test]
fn a_plugin_skill_matches_by_its_call_name() {
    let fx = Fixture::new();
    let root = fx.dir.path().join("plug");
    write_text(
        &root.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: d\n---\n",
    );
    write_text(
        &fx.dir.path().join(".claude/plugins/installed_plugins.json"),
        &json!({"plugins":{"kit@market":[{"installPath":root,"scope":"user"}]}}).to_string(),
    );
    write(
        &fx.claude("a.jsonl"),
        &[
            claude_tool("kit:review", RECENT),
            claude_tool("review", RECENT),
        ],
    );
    fx.index();
    let usage = Usage::load(&fx.cache());
    let rows = scan(&fx.homes(), None);
    let row = rows.iter().find(|r| r.plugin.is_some()).unwrap();
    assert_eq!(usage.of(row, NOW).uses90d, 1);
}

#[test]
fn codex_calls_reading_a_skill_file_count_and_other_lines_do_not() {
    let fx = Fixture::new();
    fx.skill(".codex", "vault", "vault");
    fx.skill(".claude", "vault", "vault");
    let path = "/home/u/.codex/skills/vault/SKILL.md";
    let dev = json!({"timestamp":RECENT,"type":"response_item","payload":{
        "type":"message","role":"developer",
        "content":[{"type":"input_text","text":format!("- vault: {path}")}]}});
    let ctx = json!({"timestamp":RECENT,"type":"response_item","payload":{
        "type":"message","role":"user",
        "content":[{"type":"input_text","text":format!("<environment_context>{path}</environment_context>")}]}});
    let item = json!({"timestamp":RECENT,"type":"event_msg","payload":{
        "type":"item_completed","item":{"text":format!("cat {path}")}}});
    let output = json!({"timestamp":RECENT,"type":"response_item","payload":{
        "type":"function_call_output","output":format!("ls: {path}")}});
    write(
        &fx.codex("r.jsonl"),
        &[
            codex_meta(),
            dev.to_string(),
            ctx.to_string(),
            item.to_string(),
            output.to_string(),
            codex_call(
                "function_call",
                "arguments",
                &json!({"cmd": format!("cat {path}")}).to_string(),
                RECENT,
            ),
            codex_call(
                "custom_tool_call",
                "input",
                &format!("await tools.exec({{cmd:\"sed -n 1,20p {path}\"}})"),
                "2026-10-12T08:00:00Z",
            ),
        ],
    );
    assert_eq!(fx.index().events, 2);
    let u = fx.usage("Codex", "vault");
    assert_eq!(u.uses90d, 2);
    assert_eq!(
        (u.events[0].project.as_str(), u.events[0].session.as_str()),
        ("/work/cx", "c1")
    );
    // Codex events never reach the Claude row of the same name.
    assert_eq!(fx.usage("Claude Code", "vault").uses90d, 0);
}

#[test]
fn events_older_than_90_days_are_not_counted() {
    let fx = Fixture::new();
    fx.skill(".claude", "vault", "vault");
    write(
        &fx.claude("a.jsonl"),
        &[
            claude_tool("vault", "2026-06-01T08:00:00Z"),
            claude_tool("vault", RECENT),
        ],
    );
    assert_eq!(fx.index().events, 1);
    assert_eq!(fx.usage("Claude Code", "vault").uses90d, 1);
}

#[test]
fn a_second_run_reads_nothing_and_a_grown_file_continues_at_its_offset() {
    let fx = Fixture::new();
    let file = fx.claude("a.jsonl");
    write(&file, &[claude_tool("vault", RECENT)]);
    assert!(fx.index().read_bytes > 0);
    let second = fx.index();
    assert_eq!(
        (second.read_bytes, second.events, second.scanned_files),
        (0, 1, 1)
    );

    let line = claude_tool("loop", RECENT) + "\n";
    append(&file, &line);
    let third = fx.index();
    assert_eq!(third.read_bytes, line.len() as u64);
    assert_eq!(third.events, 2);
}

#[test]
fn a_truncated_last_line_waits_until_it_is_complete() {
    let fx = Fixture::new();
    let file = fx.claude("a.jsonl");
    let line = claude_tool("vault", RECENT);
    write_text(&file, &line[..line.len() / 2]);
    assert_eq!(fx.index().events, 0);
    append(&file, &line[line.len() / 2..]);
    assert_eq!(fx.index().events, 0);
    append(&file, "\n");
    let stats = fx.index();
    assert_eq!(stats.events, 1);
    assert_eq!(stats.read_bytes, line.len() as u64 + 1);
}

#[test]
fn a_shrunk_file_is_read_again_and_a_vanished_one_is_dropped() {
    let fx = Fixture::new();
    let file = fx.claude("a.jsonl");
    write(
        &file,
        &[claude_tool("vault", RECENT), claude_tool("vault", RECENT)],
    );
    assert_eq!(fx.index().events, 2);
    write(&file, &[claude_tool("loop", RECENT)]);
    assert_eq!(fx.index().events, 1);
    fs::remove_file(&file).unwrap();
    assert_eq!(fx.index().events, 0);
    let cache: Value = serde_json::from_str(&fs::read_to_string(fx.cache()).unwrap()).unwrap();
    assert!(cache["files"].as_object().unwrap().is_empty());
}

#[test]
fn a_corrupt_cache_is_rebuilt_and_files_without_events_stay_cached() {
    let fx = Fixture::new();
    write(&fx.claude("a.jsonl"), &[claude_tool("vault", RECENT)]);
    write(
        &fx.claude("b.jsonl"),
        &[json!({"type":"summary"}).to_string()],
    );
    write_text(&fx.cache(), "{ not json");
    let stats = fx.index();
    assert!(stats.read_bytes > 0);
    assert_eq!(stats.events, 1);
    let cache: Value = serde_json::from_str(&fs::read_to_string(fx.cache()).unwrap()).unwrap();
    assert_eq!(cache["files"].as_object().unwrap().len(), 2);
    assert_eq!(fx.index().read_bytes, 0);
}

#[test]
fn codex_listings_and_writes_do_not_count_but_reads_do() {
    let fx = Fixture::new();
    fx.skill(".codex", "vault", "vault");
    let path = "/home/u/.codex/skills/vault/SKILL.md";
    let call = |cmd: String| {
        codex_call(
            "function_call",
            "arguments",
            &json!({"cmd": cmd}).to_string(),
            RECENT,
        )
    };
    let patch = json!({"timestamp":RECENT,"type":"response_item","payload":{
        "type":"custom_tool_call","name":"apply_patch",
        "input":format!("*** Begin Patch\n*** Update File: {path}\n@@\n-a\n+b\n*** End Patch")}});
    write(
        &fx.codex("r.jsonl"),
        &[
            codex_meta(),
            call(format!("ls -la {path}")),
            call(format!("find /home/u -path {path}")),
            call(format!("rg --files /home/u | grep {path}")),
            call(format!("test -e {path} && echo yes")),
            call(format!("sed -i s/a/b/ {path}")),
            patch.to_string(),
            call(format!("*** Update File: {path}")),
            call(format!("bash -lc 'cat {path}'")),
        ],
    );
    assert_eq!(fx.index().events, 1);
    assert_eq!(fx.usage("Codex", "vault").uses90d, 1);
}

#[test]
fn a_codex_plugin_skill_matches_by_its_file() {
    let fx = Fixture::new();
    let root = fx.dir.path().join(".codex/plugins/cache/market/kit/1.0.0");
    write_text(
        &root.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: d\n---\n",
    );
    write_text(
        &fx.dir.path().join(".codex/config.toml"),
        "[plugins.\"kit@market\"]\nenabled = true\n",
    );
    let path = root.join("skills/review/SKILL.md");
    write(
        &fx.codex("r.jsonl"),
        &[
            codex_meta(),
            codex_call(
                "function_call",
                "arguments",
                &json!({"cmd": format!("cat {}", path.display())}).to_string(),
                RECENT,
            ),
            // The plugin was updated since: the older version folder still counts.
            codex_call(
                "function_call",
                "arguments",
                &json!({"cmd": "cat /home/u/.codex/plugins/cache/market/kit/0.9.0/skills/review/SKILL.md"})
                    .to_string(),
                RECENT,
            ),
            // Another plugin's skill of the same name does not.
            codex_call(
                "function_call",
                "arguments",
                &json!({"cmd": "cat /home/u/.codex/plugins/cache/market/other/1.0.0/skills/review/SKILL.md"})
                    .to_string(),
                RECENT,
            ),
        ],
    );
    fx.index();
    let usage = Usage::load(&fx.cache());
    let rows = scan(&fx.homes(), None);
    let row = rows.iter().find(|r| r.plugin.is_some()).unwrap();
    assert_eq!(usage.of(row, NOW).uses90d, 2);
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_keeps_its_prior_entry() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    let file = fx.claude("a.jsonl");
    write(&file, &[claude_tool("vault", RECENT)]);
    assert_eq!(fx.index().events, 1);
    append(&file, &(claude_tool("vault", RECENT) + "\n"));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = fs::File::open(&file).is_err();
    let stats = fx.index();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    if unreadable {
        assert_eq!((stats.skipped_files, stats.events), (1, 1));
    }
}
