//! ci-crates: sushiai-agents
//! `sushiai skills` against fixture homes: the catalog, install and remove.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_sushiai");
const NAME: &str = "sushiai-artifacts";

fn skill(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Runs the binary with `home` as HOME and no tool folder settings of this machine.
fn sushiai(home: &Path, args: &[&str], stdin: &str) -> (i32, String) {
    let mut child = Command::new(BIN)
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
    )
}

fn catalog(home: &Path) -> Vec<Value> {
    let (code, out) = sushiai(home, &["skills", "--json", "--catalog"], "");
    assert_eq!(code, 0, "{out}");
    serde_json::from_str(&out).unwrap()
}

fn rows<'a>(rows: &'a [Value], name: &str) -> Vec<&'a Value> {
    rows.iter().filter(|r| r["name"] == name).collect()
}

fn harnesses(rows: &[&Value]) -> Vec<String> {
    let mut h: Vec<String> = rows
        .iter()
        .map(|r| r["harness"].as_str().unwrap().to_string())
        .collect();
    h.sort();
    h
}

#[test]
fn a_fixture_home_scans_to_one_row_per_harness_and_no_other_harness() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let old = skill(NAME, "An older copy");
    for tool in [".claude", ".codex", ".agents"] {
        write(
            &home.join(tool).join("skills").join(NAME).join("SKILL.md"),
            &old,
        );
    }
    // Other harnesses' folders are not sushiAI's to list.
    for folder in [
        ".cursor/skills",
        ".cursor/skills-cursor",
        ".gemini/antigravity/skills",
        ".windsurf/skills",
        ".opencode/skills",
        ".goose/skills",
        ".amp/skills",
        ".factory/skills",
        ".roo/skills",
        ".continue/skills",
        ".local/share/terminal-browser/app/skills",
    ] {
        write(
            &home.join(folder).join("elsewhere/SKILL.md"),
            &skill("elsewhere", "Another harness"),
        );
    }
    // One real skill linked into every folder of both harnesses.
    let real = home.join("dev/linked");
    write(
        &real.join("SKILL.md"),
        &skill("linked", "Linked everywhere"),
    );
    for tool in [".claude", ".codex", ".agents"] {
        std::os::unix::fs::symlink(&real, home.join(tool).join("skills/linked")).unwrap();
    }

    let before = catalog(home);
    assert!(rows(&before, "elsewhere").is_empty(), "{before:#?}");
    for r in &before {
        assert!(
            r["harness"] == "Claude Code" || r["harness"] == "Codex",
            "{r}"
        );
    }
    let linked = rows(&before, "linked");
    assert_eq!(harnesses(&linked), ["Claude Code", "Codex"]);
    assert!(linked.iter().all(|r| r["isDuplicate"] == false));
    // Before the installer ran, Codex sees two copies of the builtin: a real duplicate.
    let codex: Vec<&Value> = rows(&before, NAME)
        .into_iter()
        .filter(|r| r["harness"] == "Codex")
        .collect();
    assert_eq!(codex.len(), 2);
    assert!(codex.iter().all(|r| r["isDuplicate"] == true));

    let new = skill(NAME, "The current copy");
    let (code, out) = sushiai(home, &["skills", "install", NAME], &new);
    assert_eq!(code, 0, "{out}");
    let claude = home.join(".claude/skills").join(NAME).join("SKILL.md");
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), new);
    let codex_copies = [".codex", ".agents"]
        .iter()
        .filter(|t| {
            home.join(t)
                .join("skills")
                .join(NAME)
                .join("SKILL.md")
                .exists()
        })
        .count();
    assert_eq!(codex_copies, 1);

    let after = catalog(home);
    let builtin = rows(&after, NAME);
    assert_eq!(harnesses(&builtin), ["Claude Code", "Codex"]);
    assert!(builtin.iter().all(|r| r["isDuplicate"] == false));
    assert!(builtin
        .iter()
        .all(|r| r["description"] == "The current copy"));
    assert!(rows(&after, "elsewhere").is_empty());
    assert_eq!(harnesses(&rows(&after, "linked")), ["Claude Code", "Codex"]);

    // Once more changes nothing.
    let (code, out) = sushiai(home, &["skills", "install", NAME], &new);
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn install_writes_only_where_the_tool_lives_and_leaves_a_foreign_skill_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::create_dir(home.join(".claude")).unwrap();
    let text = skill(NAME, "Ours");

    let (code, out) = sushiai(home, &["skills", "install", NAME], &text);
    assert_eq!(code, 0, "{out}");
    let ours = home.join(".claude/skills").join(NAME).join("SKILL.md");
    assert_eq!(std::fs::read_to_string(&ours).unwrap(), text);
    assert!(!home.join(".codex").exists());
    assert!(!home.join(".agents").exists());

    // A skill of the same folder name that declares another name is someone else's.
    let foreign_text = skill("someone-else", "Not ours");
    let foreign = home.join(".claude/skills/sushiai-routines/SKILL.md");
    write(&foreign, &foreign_text);
    std::fs::create_dir(home.join(".codex")).unwrap();
    let shared = home.join(".agents/skills/sushiai-routines/SKILL.md");
    write(&shared, &foreign_text);
    let routines = skill("sushiai-routines", "Ours too");
    let (code, out) = sushiai(home, &["skills", "install", "sushiai-routines"], &routines);
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), foreign_text);
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), foreign_text);
    assert_eq!(
        std::fs::read_to_string(home.join(".codex/skills/sushiai-routines/SKILL.md")).unwrap(),
        routines
    );
    let (code, out) = sushiai(home, &["skills", "remove", "sushiai-routines"], "");
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), foreign_text);
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), foreign_text);
    assert!(!home.join(".codex/skills/sushiai-routines").exists());

    // Removing ours takes its folder too.
    let (code, out) = sushiai(home, &["skills", "remove", NAME], "");
    assert_eq!(code, 0, "{out}");
    assert!(!ours.parent().unwrap().exists());

    // A text that is not this skill, or a name that is not a plain token, is refused.
    let (code, _) = sushiai(home, &["skills", "install", NAME], "no frontmatter");
    assert_eq!(code, 1);
    let (code, _) = sushiai(home, &["skills", "install", "../x"], &text);
    assert_eq!(code, 1);
    assert!(!ours.exists());
}

#[test]
fn a_disabled_plugin_skill_is_listed_disabled_and_never_a_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let plugin = home.join(".claude/plugins/cache/example/tool/1.0");
    write(
        &plugin.join("skills/shared/SKILL.md"),
        &skill("shared", "Plugin copy"),
    );
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &serde_json::json!({
            "version": 2,
            "plugins": {"tool@example": [{"scope": "user", "installPath": plugin}]},
        })
        .to_string(),
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"enabledPlugins":{"tool@example":false}}"#,
    );
    write(
        &home.join(".claude/skills/shared/SKILL.md"),
        &skill("shared", "Local copy"),
    );
    // A Codex plugin that is off, with a skill of the same name.
    write(
        &home.join(".codex/plugins/cache/market/other/2.0/skills/shared/SKILL.md"),
        &skill("shared", "Codex plugin copy"),
    );
    write(
        &home.join(".codex/config.toml"),
        "[plugins.\"other@market\"]\nenabled = false\n",
    );
    // One copy for each harness is not a duplicate; two for one harness are.
    write(
        &home.join(".claude/skills/pair/SKILL.md"),
        &skill("pair", "Claude"),
    );
    write(
        &home.join(".codex/skills/pair/SKILL.md"),
        &skill("pair", "Codex"),
    );
    write(
        &home.join(".claude/skills/twin-a/SKILL.md"),
        &skill("twin", "A"),
    );
    write(
        &home.join(".claude/skills/twin-b/SKILL.md"),
        &skill("twin", "B"),
    );

    let all = catalog(home);
    let shared = rows(&all, "shared");
    assert_eq!(shared.len(), 3, "{shared:#?}");
    for r in &shared {
        assert_eq!(r["isDuplicate"], false, "{r}");
    }
    let plugins: Vec<&&Value> = shared.iter().filter(|r| r["source"] == "Plugin").collect();
    assert_eq!(plugins.len(), 2);
    for r in plugins {
        assert_eq!(r["availability"], "disabled");
        assert_eq!(r["disabledBy"], "provider");
    }
    assert!(rows(&all, "pair").iter().all(|r| r["isDuplicate"] == false));
    let twins = rows(&all, "twin");
    assert_eq!(twins.len(), 2);
    assert!(twins.iter().all(|r| r["isDuplicate"] == true));
    assert!(twins.iter().all(|r| r["duplicateKind"] == "name"));
}

#[test]
fn the_catalog_reads_the_usage_cache_and_only_the_index_command_fills_it() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write(
        &home.join(".claude/skills").join(NAME).join("SKILL.md"),
        &skill(NAME, "Artifacts"),
    );
    // A day ago, so the use stays inside the 90 day window whenever the test runs.
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 86_400_000;
    let at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let line = serde_json::json!({"type":"assistant","timestamp":at,"cwd":"/work/app","sessionId":"s1",
        "message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":NAME}}]}});
    write(
        &home.join(".claude/projects/-p/a.jsonl"),
        &format!("{line}\n"),
    );
    let run = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .env("HOME", home)
            .env("SUSHIAI_HOME", home.join(".sushiai"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let usage = |rows: &Value| rows.as_array().unwrap()[0]["usage"].clone();

    let before = run(&["skills", "--json", "--catalog"]);
    assert_eq!(usage(&before)["uses90d"], 0);
    assert!(usage(&before)["lastUsedAt"].is_null());
    assert!(usage(&before)["indexedAt"].is_null());
    assert!(before[0].get("accessedAt").is_none());

    let stats = run(&["skills", "usage", "--index", "--json"]);
    assert_eq!(stats["scannedFiles"], 1);
    assert_eq!(stats["skippedFiles"], 0);
    assert_eq!(stats["events"], 1);
    assert!(stats["readBytes"].as_u64().unwrap() > 0);
    assert!(stats["durationMs"].is_u64());
    assert_eq!(
        run(&["skills", "usage", "--index", "--json"])["readBytes"],
        0
    );

    let after = run(&["skills", "--json", "--catalog"]);
    let u = usage(&after);
    assert_eq!(u["lastUsedAt"], at.as_str());
    assert_eq!(u["events"][0]["project"], "/work/app");
    assert!(u["indexedAt"].is_string());
    // The routines listing keeps its shape.
    let list = run(&["skills", "--json"]);
    assert!(list[0].get("usage").is_none());
}
