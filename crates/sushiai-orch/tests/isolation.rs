//! Task-run isolation, using only temporary daemon homes and synthetic secrets.

mod common;

use common::*;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const HARNESS: &str = r#"#!/bin/sh
[ "$1" = --version ] && { echo 'codex-cli 0.1'; exit 0; }
printf '%s\n' "$@" > "$CAPTURE_DIR/$ORCHD_TASK.argv"
for arg in "$@"; do
  if [ "$prev" = --mcp-config ]; then
    cp "$arg" "$CAPTURE_DIR/$ORCHD_TASK.mcp.json"
    if [ "$(uname)" = Darwin ]; then
      stat -f '%Lp' "$arg" > "$CAPTURE_DIR/$ORCHD_TASK.mode"
    else
      stat -c '%a' "$arg" > "$CAPTURE_DIR/$ORCHD_TASK.mode"
    fi
  fi
  prev="$arg"
done
printf '%s' "${PROJECT_TOKEN-unset}" > "$CAPTURE_DIR/$ORCHD_TASK.env"
cat > /dev/null
echo changed > CHANGED_MARKER.txt
if [ "$1" = exec ]; then
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}}'
else
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-isolation"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
fi
"#;

fn configure(daemon: &Daemon, route: &str) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["maxAttempts"] = json!(1);
    for tier in ["mechanical", "standard", "hard"] {
        settings["tiers"][tier] = json!(route);
    }
    daemon.request("settings.set", json!({"settings": settings}));
}

fn create(daemon: &Daemon, repo: &Path, extra: Value) -> Value {
    let mut params = json!({
        "repo": repo, "title": "Isolation", "goal": "Make a change",
        "verify": ["true"], "start": false, "land": false,
    });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    daemon.request("task.create", params)
}

fn run(daemon: &Daemon, task: &Value) -> Value {
    let id = task["id"].as_str().unwrap();
    daemon.request("task.start", json!({"id": id}));
    poll_task_status(daemon, id)
}

#[test]
fn full_bridges_are_removed_from_claude_configs_and_codex_argv() {
    let scripts = tempfile::tempdir().unwrap();
    let capture = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(scripts.path(), "harness.sh", HARNESS);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", harness.to_str().unwrap()),
        ("CAPTURE_DIR", capture.path().to_str().unwrap()),
    ]);
    let repo = init_git_repo();
    let full = json!({"command": sushiai_bin(), "args": ["mcp"], "codex": true});
    let readonly =
        json!({"command": "/opt/bin/sushiai", "args": ["mcp", "--read-only"], "codex": true});
    fs::write(
        repo.path().join(".mcp.json"),
        json!({"mcpServers": {
            "repo-full": full, "repo-readonly": readonly,
            "repo-safe": {"command": "example-server"}
        }})
        .to_string(),
    )
    .unwrap();
    git_out(repo.path(), &["add", ".mcp.json"]);
    git_out(repo.path(), &["commit", "-q", "-m", "project MCP"]);
    for (route, claude) in [("claude-sonnet", true), ("codex", false)] {
        configure(&daemon, route);
        let task = create(
            &daemon,
            repo.path(),
            json!({"mcp": {"mcpServers": {
                "task-full": full, "task-readonly": readonly,
                "safe": {"command": "example-server", "codex": true},
                "unrelated": {"command": "sushiai", "args": ["orch", "status"], "codex": true},
                "scoped": {"command": "sushiai", "args": ["mcp", "--task", "another-id"], "codex": true}
            }}}),
        );
        let id = task["id"].as_str().unwrap();
        let settled = run(&daemon, &task);
        assert_eq!(settled["status"], "done", "{settled}");
        assert_eq!(
            settled["attempts"][0]["harness"],
            if claude { "claude" } else { "codex" },
            "{settled}"
        );
        if claude {
            let config: Value = serde_json::from_slice(
                &fs::read(capture.path().join(format!("{id}.mcp.json"))).unwrap(),
            )
            .unwrap();
            let servers = config["mcpServers"].as_object().unwrap();
            for name in ["repo-full", "repo-readonly", "task-full", "task-readonly"] {
                assert!(!servers.contains_key(name), "{config}");
            }
            for name in ["repo-safe", "safe", "unrelated", "scoped"] {
                assert!(servers.contains_key(name), "{config}");
            }
            assert_eq!(
                servers["sushiai-messages"]["args"],
                json!(["mcp", "--task", id])
            );
        } else {
            let argv = fs::read_to_string(capture.path().join(format!("{id}.argv"))).unwrap();
            for name in ["repo-full", "repo-readonly", "task-full", "task-readonly"] {
                assert!(!argv.contains(&format!("mcp_servers.{name}.")), "{argv}");
            }
            for name in ["safe", "unrelated", "scoped"] {
                assert!(argv.contains(&format!("mcp_servers.{name}.")), "{argv}");
            }
            let native = daemon.request("settings.get", json!({}))["sandbox"] == "native";
            assert_eq!(
                argv.contains("mcp_servers.sushiai-messages."),
                !native,
                "{argv}"
            );
            if !native {
                assert!(
                    argv.contains(&format!(
                        "mcp_servers.sushiai-messages.args=[\"mcp\",\"--task\",\"{id}\"]"
                    )),
                    "{argv}"
                );
            }
        }
        assert!(!daemon
            .data_dir()
            .join("tasks")
            .join(id)
            .join("runs/1/mcp.json")
            .exists());
    }
}

#[test]
fn task_create_requires_a_matching_project_folder_before_creating_anything() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let other = init_git_repo();
    let params = json!({"repo": repo.path(), "projectId": "foreign-project", "title": "Reject", "goal": "Reject", "start": false});
    let branches = git_out(repo.path(), &["branch", "--list"]);
    let worktrees = git_out(repo.path(), &["worktree", "list", "--porcelain"]);
    for secrets in [
        json!({}),
        json!({"projectRepos": {other.path().to_str().unwrap(): "foreign-project"}}),
        json!({"projectRepos": {repo.path().to_str().unwrap(): "other-project"}}),
    ] {
        daemon.request("secrets.set", secrets);
        let error = daemon.request_error("task.create", params.clone());
        assert!(
            error.contains("foreign-project") && error.contains(repo.path().to_str().unwrap()),
            "{error}"
        );
        assert_eq!(daemon.request("task.list", json!({})), json!([]));
        assert_eq!(git_out(repo.path(), &["branch", "--list"]), branches);
        assert_eq!(
            git_out(repo.path(), &["worktree", "list", "--porcelain"]),
            worktrees
        );
    }
    for folder in [repo.path(), repo.path().parent().unwrap()] {
        daemon.request(
            "secrets.set",
            json!({"projectRepos": {folder.to_str().unwrap(): "matching-project"}}),
        );
        let task = create(
            &daemon,
            repo.path(),
            json!({"projectId": "matching-project"}),
        );
        assert_eq!(task["projectId"], "matching-project");
        let inferred = create(&daemon, repo.path(), json!({}));
        assert_eq!(inferred["projectId"], "matching-project");
    }
}

#[test]
fn a_stored_project_mismatch_fails_without_starting_the_harness() {
    let scripts = tempfile::tempdir().unwrap();
    let capture = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(scripts.path(), "harness.sh", HARNESS);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("CAPTURE_DIR", capture.path().to_str().unwrap()),
    ]);
    configure(&daemon, "claude-sonnet");
    let repo = init_git_repo();
    daemon.request(
        "secrets.set",
        json!({
            "projects": {"old-project": {"PROJECT_TOKEN": "invented-old-project-secret"}},
            "projectRepos": {repo.path().to_str().unwrap(): "old-project"}
        }),
    );
    let task = create(
        &daemon,
        repo.path(),
        json!({"projectId": "old-project", "mcp": {"mcpServers": {
            "example": {"command": "server", "env": {"TOKEN": "${PROJECT_TOKEN}"}}
        }}}),
    );
    daemon.request(
        "secrets.set",
        json!({
            "projects": {"old-project": {"PROJECT_TOKEN": "invented-old-project-secret"}},
            "projectRepos": {repo.path().to_str().unwrap(): "new-project"}
        }),
    );
    let settled = run(&daemon, &task);
    assert_eq!(settled["status"], "waiting", "{settled}");
    assert_eq!(settled["attempts"][0]["status"], "failed", "{settled}");
    assert!(
        settled["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("projectId \"old-project\"")),
        "{settled}"
    );
    assert_eq!(fs::read_dir(capture.path()).unwrap().count(), 0);
}

fn assert_no_secret(dir: &Path, secret: &str) {
    // The daemon may remove a temporary file or folder while this walks the
    // tree; what is gone holds no secret.
    let entries = match fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        entries => entries.unwrap(),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            assert_no_secret(&path, secret);
        } else {
            let bytes = match fs::read(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                read => read.unwrap(),
            };
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
                "secret in {}",
                path.display()
            );
        }
    }
}

#[test]
fn run_mcp_is_private_during_the_run_and_leaves_no_project_secret_afterwards() {
    let scripts = tempfile::tempdir().unwrap();
    let capture = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(scripts.path(), "harness.sh", HARNESS);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("CAPTURE_DIR", capture.path().to_str().unwrap()),
    ]);
    configure(&daemon, "claude-sonnet");
    let repo = init_git_repo();
    let secret = "invented-isolation-secret-42";
    daemon.request(
        "secrets.set",
        json!({
            "projects": {"project": {"PROJECT_TOKEN": secret}},
            "projectRepos": {repo.path().to_str().unwrap(): "project"}
        }),
    );
    let task = create(
        &daemon,
        repo.path(),
        json!({"projectId": "project", "mcp": {"mcpServers": {
            "example": {"command": "server", "env": {"TOKEN": "${PROJECT_TOKEN}"}}
        }}}),
    );
    let id = task["id"].as_str().unwrap();
    let settled = run(&daemon, &task);
    assert_eq!(settled["status"], "done", "{settled}");
    let config: Value =
        serde_json::from_slice(&fs::read(capture.path().join(format!("{id}.mcp.json"))).unwrap())
            .unwrap();
    assert_eq!(config["mcpServers"]["example"]["env"]["TOKEN"], secret);
    assert_eq!(
        fs::read_to_string(capture.path().join(format!("{id}.env"))).unwrap(),
        secret
    );
    assert_eq!(
        fs::read_to_string(capture.path().join(format!("{id}.mode")))
            .unwrap()
            .trim(),
        "600"
    );
    let runs = daemon.data_dir().join("tasks").join(id).join("runs");
    assert!(!runs.join("1/mcp.json").exists());
    assert_no_secret(&runs, secret);
}

#[test]
fn secrets_set_redacts_old_run_mcp_files_once_and_makes_them_private() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), json!({}));
    let task_dir = daemon
        .data_dir()
        .join("tasks")
        .join(task["id"].as_str().unwrap());
    let run = task_dir.join("runs/1");
    fs::create_dir_all(run.join("b/nested")).unwrap();
    let old = json!({"mcpServers": {"example": {
        "command": "server", "args": ["invented-secret-long", "invented-secret"],
        "url": "https://example.invalid/invented-mcp-secret",
        "env": {"TOKEN": "invented-secret-long", "UNKNOWN": "unrecognized-secret", "NUMBER": 1},
        "headers": {"Authorization": "Bearer unrecognized-secret"}
    }}});
    let paths = [
        run.join("mcp.json"),
        run.join("b/mcp.json"),
        run.join("b/nested/mcp.json"),
    ];
    for path in &paths {
        fs::write(path, old.to_string()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    let invalid = run.join("b/nested/broken/mcp.json");
    fs::create_dir_all(invalid.parent().unwrap()).unwrap();
    fs::write(&invalid, "invalid JSON").unwrap();
    fs::set_permissions(&invalid, fs::Permissions::from_mode(0o644)).unwrap();
    let params = json!({
        "projects": {"project": {"LONG": "invented-secret-long", "SHORT": "invented-secret"}},
        "projectMcp": {"project": {"MCP_TOKEN": "invented-mcp-secret"}},
        "projectRepos": {repo.path().to_str().unwrap(): "project"}
    });
    daemon.request("secrets.set", params.clone());
    for path in &paths {
        let config: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let server = &config["mcpServers"]["example"];
        assert_eq!(server["args"], json!(["${LONG}", "${SHORT}"]));
        assert_eq!(server["url"], "https://example.invalid/${MCP_TOKEN}");
        assert_eq!(
            server["env"],
            json!({"TOKEN": "${TOKEN}", "UNKNOWN": "${UNKNOWN}", "NUMBER": 1})
        );
        assert_eq!(server["headers"]["Authorization"], "${Authorization}");
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(fs::read_to_string(&invalid).unwrap(), "invalid JSON");
    assert_eq!(
        fs::metadata(&invalid).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(daemon.data_dir().join("run-mcp-redacted").is_file());
    let later = run.join("b/later/mcp.json");
    fs::create_dir_all(later.parent().unwrap()).unwrap();
    fs::write(&later, old.to_string()).unwrap();
    fs::set_permissions(&later, fs::Permissions::from_mode(0o644)).unwrap();
    daemon.request("secrets.set", params);
    assert_eq!(fs::read_to_string(&later).unwrap(), old.to_string());
    assert_eq!(
        fs::metadata(&later).unwrap().permissions().mode() & 0o777,
        0o644
    );
}
