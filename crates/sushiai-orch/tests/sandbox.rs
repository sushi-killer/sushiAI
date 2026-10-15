//! Native task isolation. All writable state belongs to disposable test homes.

mod common;

use common::*;
use serde_json::json;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use sushiai_orch::sandbox::to_sbpl as task_sandbox_profile;
use sushiai_orch::{
    harness::shell_quote,
    sandbox::{Inputs, TaskSandbox},
};

#[cfg(target_os = "macos")]
const SANDBOX_EXEC: &str = "sandbox-exec";
#[cfg(not(target_os = "macos"))]
const SANDBOX_EXEC: &str = "unavailable-native-sandbox";
#[cfg(target_os = "macos")]
const LIBRARY_CACHE: &str = "Library/Caches";
#[cfg(not(target_os = "macos"))]
const LIBRARY_CACHE: &str = ".cache";

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn available() -> bool {
    let reason = if !cfg!(target_os = "macos") {
        Some("sandbox-exec requires macOS")
    } else if native_sandbox_unavailable() {
        Some("sandbox-exec cannot apply a profile (nested or unavailable Seatbelt)")
    } else if !checkout().join("node_modules/electron").is_dir() {
        Some("Electron is not installed in the sushiAI checkout")
    } else {
        None
    };
    if let Some(reason) = reason {
        eprintln!("SKIP sandbox: {reason}");
        return false;
    }
    true
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    worktree: PathBuf,
    other_worktree: PathBuf,
    user: PathBuf,
    data: PathBuf,
    run: PathBuf,
    cargo: PathBuf,
    rustup: PathBuf,
    socket: PathBuf,
    policy: TaskSandbox,
}

impl Fixture {
    fn new() -> Self {
        Self::with_root("")
    }

    fn with_root(worktree_root: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let repo = root.join("projects/repo");
        let worktree = sushiai_orch::git::worktree::worktree_path(
            &repo,
            &root.join("sushiai"),
            worktree_root,
            "task/own",
        );
        let other_worktree = worktree.with_file_name("task-other");
        let user = root.join("user");
        let data = root.join("sushiai/orchestrator");
        let run = data.join("tasks/own/runs/1");
        let socket = root.join("sushiai/daemon.sock");
        for dir in [
            repo.clone(),
            root.join("projects/sibling"),
            root.join("sushiai/worktrees.noindex/other-repo/task-other"),
            other_worktree.clone(),
            root.join("old-worktrees/task-other"),
            run.clone(),
            data.join("tasks/other/runs/1"),
            user.join(".claude/projects"),
            user.join(".codex/sessions"),
            user.join(".codex/archived_sessions"),
            user.join(LIBRARY_CACHE),
            user.join(".cache"),
            user.join(".npm"),
            root.join("tmp"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        for path in [
            root.join("projects/sibling/file"),
            root.join("projects/sibling/.env"),
            root.join("sushiai/worktrees.noindex/other-repo/task-other/.env"),
            other_worktree.join(".env"),
            root.join("old-worktrees/task-other/.env"),
            data.join("tasks/other/runs/1/secret"),
            user.join(".claude/projects/secret"),
            user.join(".codex/sessions/secret"),
            user.join(".codex/archived_sessions/secret"),
        ] {
            fs::write(path, "synthetic secret").unwrap();
        }
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("HOME", &user)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "sandbox test"]);
        git(&["config", "user.email", "sandbox@example.invalid"]);
        fs::write(repo.join("README.md"), "base").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        git(&["worktree", "add", "-qb", "task", worktree.to_str().unwrap()]);
        let (cargo, rustup) = sandbox_toolchain_homes(&user);
        let policy = TaskSandbox::build(Inputs {
            repo_root: &repo,
            worktree: &worktree,
            run_dir: &run,
            data_dir: &data,
            sushiai_home: &root.join("sushiai"),
            worktree_root,
            home: &user,
            daemon_socket: &socket,
            cargo_home: Some(&cargo),
            rustup_home: Some(&rustup),
            tmp_dir: Some(&root.join("tmp")),
            npm_cache: Some(&user.join(".npm")),
            known_project_folders: &[root.join("projects")],
            known_task_worktrees: &[root.join("old-worktrees/task-other"), worktree.clone()],
        });
        Self {
            _temp: temp,
            root,
            repo,
            worktree,
            other_worktree,
            user,
            data,
            run,
            cargo,
            rustup,
            socket,
            policy,
        }
    }

    fn run(&self, script: &str) -> Output {
        Command::new(SANDBOX_EXEC)
            .args([
                "-p",
                &task_sandbox_profile(&self.policy),
                "/bin/sh",
                "-c",
                script,
            ])
            .current_dir(&self.worktree)
            .env("HOME", &self.user)
            .env("SUSHIAI_HOME", self.root.join("sushiai"))
            .env("CODEX_HOME", self.user.join(".codex"))
            .env("CARGO_HOME", &self.cargo)
            .env("RUSTUP_HOME", &self.rustup)
            .env("TMPDIR", self.root.join("tmp"))
            .env("RUSTC_WRAPPER", "")
            .envs(&self.policy.check_env)
            .output()
            .unwrap()
    }
}

#[test]
fn profile_denies_foreign_projects_runs_sessions_and_daemon_connect() {
    let fixture = Fixture::new();
    let sbpl = task_sandbox_profile(&fixture.policy);
    let sessions = json!(fixture.user.join(".codex/sessions").to_string_lossy()).to_string();
    assert!(sbpl.contains(&format!("(deny file-read* (subpath {sessions}))")));
    assert!(sbpl.contains(&format!("(allow file-read-metadata (subpath {sessions}))")));
    if !available() {
        return;
    }
    let existing_codex_session_dir = fixture.user.join(".codex/sessions/2026/10/12");
    fs::create_dir_all(&existing_codex_session_dir).unwrap();
    fs::write(
        existing_codex_session_dir.join("prior.jsonl"),
        "private session",
    )
    .unwrap();
    for path in [
        fixture.root.join("projects/sibling/file"),
        fixture.root.join("projects/sibling/.env"),
        fixture
            .root
            .join("sushiai/worktrees.noindex/other-repo/task-other/.env"),
        fixture.other_worktree.join(".env"),
        fixture.root.join("old-worktrees/task-other/.env"),
        fixture.data.join("tasks/other/runs/1/secret"),
        fixture.user.join(".claude/projects/secret"),
        fixture.user.join(".codex/sessions/secret"),
        existing_codex_session_dir.join("prior.jsonl"),
        fixture.user.join(".codex/archived_sessions/secret"),
    ] {
        let out = fixture.run(&format!("cat {}", shell_quote(path.to_str().unwrap())));
        assert!(!out.status.success(), "read {} succeeded", path.display());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Operation not permitted") || stderr.contains("Permission denied"),
            "{stderr}"
        );
    }
    for dir in [&fixture.data, fixture.worktree.parent().unwrap()] {
        let out = fixture.run(&format!("ls {}", shell_quote(dir.to_str().unwrap())));
        assert!(!out.status.success(), "listing {} succeeded", dir.display());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Operation not permitted") || stderr.contains("Permission denied"),
            "{stderr}"
        );
    }
    let codex_session_setup = fixture.run(&format!(
        "stat {} && mkdir -p {}/new-session",
        shell_quote(existing_codex_session_dir.to_str().unwrap()),
        shell_quote(existing_codex_session_dir.to_str().unwrap())
    ));
    assert!(
        codex_session_setup.status.success(),
        "Codex could not stat/create under its existing session directory: {}",
        String::from_utf8_lossy(&codex_session_setup.stderr)
    );
    let _listener = UnixListener::bind(&fixture.socket).unwrap();
    let js = format!(
        "const net = require('node:net'); const s = net.connect({}); s.on('connect', () => process.exit(2)); s.on('error', e => {{ console.log(e.code); process.exit(e.code === 'EPERM' || e.code === 'EACCES' ? 0 : 3); }});",
        json!(fixture.socket)
    );
    let out = fixture.run(&format!("node -e {}", shell_quote(&js)));
    assert!(
        out.status.success(),
        "connect: {:?} {} {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn profile_denies_custom_worktrees_and_keeps_own_worktree_usable() {
    if !available() {
        return;
    }
    let fixture = Fixture::with_root("task-worktrees");
    let path = fixture.other_worktree.join(".env");
    let out = fixture.run(&format!("cat {}", shell_quote(path.to_str().unwrap())));
    assert!(!out.status.success(), "read {} succeeded", path.display());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Operation not permitted") || stderr.contains("Permission denied"),
        "{stderr}"
    );
    let out = fixture.run(
        "printf test > own-file && cat own-file && git status --porcelain && git add own-file",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn profile_allows_worktree_git_own_run_tmp_and_caches() {
    if !available() {
        return;
    }
    let fixture = Fixture::new();
    for dir in [
        &fixture.worktree,
        &fixture.run,
        &fixture.root.join("tmp"),
        &fixture.user.join(".cache"),
        &fixture.user.join(".npm"),
        &fixture.user.join(LIBRARY_CACHE),
        &fixture.cargo,
        &fixture.rustup,
    ] {
        let file = dir.join("sandbox-write");
        let quoted = shell_quote(file.to_str().unwrap());
        let out = fixture.run(&format!("printf test > {quoted} && cat {quoted}"));
        assert!(
            out.status.success(),
            "{}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"test");
    }
    assert!(fixture
        .policy
        .write_roots
        .contains(&fixture.repo.join(".git")));
    let out = fixture
        .run("git status --porcelain && git add sandbox-write && git diff --cached --name-only");
    assert!(
        out.status.success(),
        "git: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("sandbox-write"));
}

const CODEX: &str = r#"#!/bin/sh
[ "$1" = --version ] && { echo 'codex-cli 0.1'; exit 0; }
printf '%s\n' "$@" > implement.argv
node -e 'const fs=require("node:fs"); for (const path of [process.env.SIBLING_SECRET, fs.readFileSync("foreign-worktree-path", "utf8")]) { let denied=false; try { fs.readFileSync(path) } catch(e) { if (!["EPERM","EACCES"].includes(e.code)) throw e; denied=true } if (denied !== (process.env.EXPECT_NATIVE === "1")) process.exit(1) }'
[ "$?" = 0 ] || exit 1
cat > /dev/null
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}}'
"#;

fn daemon_task(native: bool) {
    if !available() {
        return;
    }
    let fixtures = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(fixtures.path(), "codex.sh", CODEX);
    let user = fixtures.path().join("user");
    let (cargo, rustup) = sandbox_toolchain_homes(&user);
    let repo = init_git_repo();
    let sibling = fixtures.path().join("sibling");
    fs::create_dir(&sibling).unwrap();
    fs::write(sibling.join(".env"), "synthetic secret").unwrap();
    fs::write(sibling.join(".gitignore"), ".env\n").unwrap();
    fs::write(sibling.join(".worktreeinclude"), ".env\n").unwrap();
    git_out(&sibling, &["init", "-q"]);
    git_out(&sibling, &["add", "."]);
    git_out(
        &sibling,
        &[
            "-c",
            "user.name=sandbox test",
            "-c",
            "user.email=sandbox@example.invalid",
            "commit",
            "-qm",
            "base",
        ],
    );
    let sibling_secret = sibling.join(".env");
    let electron = checkout().join("node_modules/electron");
    let npm_cache = user.join(".npm");
    fs::write(
        repo.path().join("package.json"),
        json!({"scripts": {
            "ci": "cargo build && node --test policy.test.cjs", "test:desktop": "node desktop.cjs"
        }})
        .to_string(),
    )
    .unwrap();
    fs::write(repo.path().join("Cargo.toml"), "[package]\nname = \"sandbox-check\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n").unwrap();
    fs::write(repo.path().join(".gitignore"), "target/\n").unwrap();
    fs::create_dir(repo.path().join("src")).unwrap();
    fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(repo.path().join("policy.test.cjs"), r#"const fs = require('node:fs');
const assert = require('node:assert/strict');
const {test} = require('node:test');
test('task policy and check environment', () => {
  const native = process.env.EXPECT_NATIVE === '1';
  assert.equal(process.env.ELECTRON_DISABLE_SANDBOX === '1', native);
  for (const path of [process.env.SIBLING_SECRET, fs.readFileSync('foreign-worktree-path', 'utf8'), process.env.SUSHIAI_HOME + '/orchestrator/tasks/foreign/runs/1/secret', process.env.HOME + '/.claude/projects/secret', process.env.HOME + '/.codex/sessions/secret']) {
    if (native) assert.throws(() => fs.readFileSync(path), e => ['EPERM', 'EACCES'].includes(e.code));
    else assert.equal(fs.readFileSync(path, 'utf8'), 'synthetic secret');
  }
  fs.writeFileSync('ci-passed', 'yes');
});
"#).unwrap();
    fs::write(repo.path().join("desktop.cjs"), r#"const {spawnSync} = require('node:child_process');
const fs = require('node:fs');
const electron = require(process.env.TEST_ELECTRON);
const out = spawnSync(electron, ['hidden-electron.cjs'], {env: {...process.env, SUSHIAI_TEST_WINDOW: 'hidden'}, encoding: 'utf8'});
if (out.status !== 0) { console.error(out.stdout, out.stderr); process.exit(1); }
fs.writeFileSync('desktop-passed', 'yes');
"#).unwrap();
    fs::write(repo.path().join("hidden-electron.cjs"), r#"const {app, BrowserWindow} = require('electron');
const fs = require('node:fs');
if (process.env.SUSHIAI_TEST_WINDOW !== 'hidden') process.exit(2);
const userData = process.env.HOME + '/.cache/sandbox-electron';
fs.mkdirSync(userData, {recursive: true});
app.setPath('userData', userData);
app.commandLine.appendSwitch('password-store', 'basic');
app.dock?.hide();
app.whenReady().then(async () => {
  const window = new BrowserWindow({show: false, webPreferences: {sandbox: true}});
  window.webContents.on('render-process-gone', () => app.exit(3));
  await window.loadURL('data:text/html,<p>Sandbox smoke</p>');
  if (window.isVisible() || BrowserWindow.getFocusedWindow() !== null) return app.exit(4);
  const text = await window.webContents.executeJavaScript('document.querySelector("p").textContent');
  app.exit(text === 'Sandbox smoke' ? 0 : 5);
}).catch(error => { console.error(error); app.exit(6); });
"#).unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-qm", "checks"]);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CODEX_BIN", harness.to_str().unwrap()),
        ("CARGO_HOME", cargo.to_str().unwrap()),
        ("RUSTUP_HOME", rustup.to_str().unwrap()),
        ("RUSTC_WRAPPER", ""),
        ("npm_config_cache", npm_cache.to_str().unwrap()),
        ("SIBLING_SECRET", sibling_secret.to_str().unwrap()),
        ("EXPECT_NATIVE", if native { "1" } else { "0" }),
        ("TEST_ELECTRON", electron.to_str().unwrap()),
        ("ELECTRON_DISABLE_SANDBOX", ""),
    ]);
    let home = daemon.home.path().join("user");
    for path in [
        daemon.data_dir().join("tasks/foreign/runs/1/secret"),
        home.join(".claude/projects/secret"),
        home.join(".codex/sessions/secret"),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "synthetic secret").unwrap();
    }
    daemon.request(
        "secrets.set",
        json!({"projectRepos": { sibling.to_str().unwrap(): "other-project" }}),
    );
    let mut settings = daemon.request("settings.get", json!({}));
    settings["sandbox"] = json!("native");
    settings["review"] = json!("");
    settings["maxAttempts"] = json!(1);
    settings["verifyTimeoutSecs"] = json!(120);
    settings["worktreeRoot"] = json!(fixtures.path().join("retained-worktrees"));
    for tier in ["mechanical", "standard", "hard"] {
        settings["tiers"][tier] = json!("codex");
    }
    if !native {
        settings["sandboxRepos"] = json!({repo.path().to_str().unwrap(): "host"});
    }
    daemon.request("settings.set", json!({"settings": settings}));
    let foreign = daemon.request("task.create", json!({
        "repo": sibling, "title": "Foreign", "goal": "Hold a synthetic ignored secret", "tier": "mechanical", "land": false, "start": false,
    }));
    let foreign_secret = Path::new(foreign["worktree"].as_str().unwrap()).join(".env");
    assert_eq!(
        fs::read_to_string(&foreign_secret).unwrap(),
        "synthetic secret"
    );
    fs::write(
        repo.path().join("foreign-worktree-path"),
        foreign_secret.to_str().unwrap(),
    )
    .unwrap();
    git_out(repo.path(), &["add", "foreign-worktree-path"]);
    git_out(repo.path(), &["commit", "-qm", "worktree fixture"]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["worktreeRoot"] = json!(".task-worktrees");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request("task.create", json!({
        "repo": repo.path(), "title": "Sandbox", "goal": "Make a change", "tier": "mechanical", "land": false,
        "verify": ["npm run ci"], "finalVerify": ["npm run test:desktop"], "start": true,
    }));
    let done = poll_task_status(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    // Done tasks release their checkout; inspect the committed result.
    let branch = done["branch"].as_str().unwrap();
    let argv = git_out(repo.path(), &["show", &format!("{branch}:implement.argv")]);
    assert_eq!(argv.contains("danger-full-access"), native, "{argv}");
    assert_eq!(argv.contains("workspace-write"), !native, "{argv}");
    assert_eq!(
        argv.contains("mcp_servers.sushiai-messages."),
        !native,
        "{argv}"
    );
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{branch}:ci-passed")]),
        "yes"
    );
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{branch}:desktop-passed")]),
        "yes"
    );
    assert_eq!(
        daemon.request("settings.get", json!({}))["sandbox"],
        "native"
    );
}

#[test]
fn native_daemon_task_builds_tests_and_launches_hidden_electron() {
    daemon_task(true);
}

#[test]
fn host_repo_override_applies_to_implement_base_verify_and_final_checks() {
    daemon_task(false);
}

#[test]
#[ignore = "runs the real repository suites; run once from an unsandboxed shell"]
fn real_repo_sandbox_checks() {
    if !available() {
        return;
    }
    let Some(repo) = std::env::var_os("SUSHIAI_SANDBOX_REPO").map(PathBuf::from) else {
        eprintln!("SKIP sandbox: SUSHIAI_SANDBOX_REPO is not set");
        return;
    };
    let fixture = Fixture::new();
    let policy = TaskSandbox::build(Inputs {
        repo_root: &repo,
        worktree: &repo,
        run_dir: &fixture.run,
        data_dir: &fixture.data,
        sushiai_home: &fixture.root.join("sushiai"),
        worktree_root: "",
        home: &fixture.user,
        daemon_socket: &fixture.socket,
        cargo_home: Some(&fixture.cargo),
        rustup_home: Some(&fixture.rustup),
        tmp_dir: Some(&fixture.root.join("tmp")),
        npm_cache: Some(&fixture.user.join(".npm")),
        known_project_folders: &[],
        known_task_worktrees: &[],
    });
    for check in [
        "npm run ci",
        "npm run test:desktop",
        "cargo build -p sushiai",
    ] {
        let mut command = Command::new(SANDBOX_EXEC);
        command
            .args(["-p", &task_sandbox_profile(&policy), "/bin/sh", "-c", check])
            .current_dir(&repo)
            .env("HOME", &fixture.user)
            .env("SUSHIAI_HOME", fixture.root.join("sushiai"))
            .env("CODEX_HOME", fixture.user.join(".codex"))
            .env("CARGO_HOME", &fixture.cargo)
            .env("RUSTUP_HOME", &fixture.rustup)
            .env("TMPDIR", fixture.root.join("tmp"))
            .env("npm_config_cache", fixture.user.join(".npm"))
            .env("SUSHIAI_TEST_WINDOW", "hidden");
        policy.apply_check_env(&mut command);
        let out = command.output().unwrap();
        eprintln!(
            "{check}: {}\n{}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success(),
            "{check} failed under the task profile"
        );
    }
}
