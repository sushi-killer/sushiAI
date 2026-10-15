use std::path::{Path, PathBuf};
use std::time::Duration;

use sushiai_orch::harness::compiler_cache::{CacheRun, CompilerCache, SERVER_PORT};
use sushiai_orch::verify::command::run_verify_command;
use sushiai_orch::verify::command::VerifyEnvironment;

/// Written by a child `sh`, never through a descriptor of this process: a
/// test thread that forks while it is open for writing makes the next exec
/// of the script fail with ETXTBSY on Linux, and `start_server` ignores that.
fn script(dir: &Path, name: &str, body: &str) {
    let status = std::process::Command::new("sh")
        .args([
            "-c",
            r#"printf '#!/bin/sh\n%s\n' "$1" > "$2" && chmod 755 "$2""#,
            "sh",
        ])
        .arg(body)
        .arg(dir.join(name))
        .status()
        .unwrap();
    assert!(status.success());
}

fn assert_no_cache_env(command: &tokio::process::Command) {
    for (key, _) in command.as_std().get_envs() {
        assert_ne!(key, "RUSTC_WRAPPER");
        assert!(!key.to_string_lossy().starts_with("SCCACHE_"), "{key:?}");
    }
}

async fn verify(wt: &Path, env: &VerifyEnvironment, script: &str) {
    let command = env.build_command(wt, script, None, "");
    assert!(!command
        .as_std()
        .get_envs()
        .any(|(k, _)| k == "CARGO_TARGET_DIR"));
    let result = run_verify_command(
        wt,
        script,
        command,
        Duration::from_secs(60),
        std::future::pending(),
    )
    .await;
    assert_eq!(result.code, Some(0), "{}", result.tail);
}

#[tokio::test]
async fn absent_sccache_leaves_verify_env_and_decisions_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let env = VerifyEnvironment::with_cache(
        tmp.path(),
        tmp.path().display().to_string(),
        tmp.path().join("cache"),
        SERVER_PORT,
        false,
    );
    assert!(env.compiler_cache.is_none());
    let command = env.build_command(tmp.path(), "printf ok", None, "base");
    assert_no_cache_env(&command);
    let gate = CacheRun::begin(env.compiler_cache.clone()).await;
    verify(tmp.path(), &env, "printf ok").await;
    let decisions: Vec<String> = gate.finish("verify").await.into_iter().collect();
    assert!(decisions.is_empty());
    assert!(!tmp.path().join("cache").exists());
}

#[tokio::test]
async fn setup_opt_out_disables_wrapper_stats_and_server_even_with_sccache() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".sushiai")).unwrap();
    std::fs::write(
        tmp.path().join(".sushiai/setup.json"),
        r#"{"compilerCache":false}"#,
    )
    .unwrap();
    script(
        tmp.path(),
        "sccache",
        "echo called >> \"$SCCACHE_DIR/calls\"; exit 99",
    );
    let env = VerifyEnvironment::with_cache(
        tmp.path(),
        tmp.path().display().to_string(),
        tmp.path().to_path_buf(),
        SERVER_PORT,
        false,
    );
    assert!(env.compiler_cache.is_none());
    assert_no_cache_env(&env.build_command(tmp.path(), "true", None, ""));
    let gate = CacheRun::begin(env.compiler_cache.clone()).await;
    verify(tmp.path(), &env, "true").await;
    assert_eq!(gate.finish("final checks").await, None);
    assert!(!tmp.path().join("calls").exists());

    std::fs::write(
        tmp.path().join(".sushiai/setup.json"),
        r#"{"compilerCache":false,"setup":"true"}"#,
    )
    .unwrap();
    sushiai_orch::worktree_env::run_setup(
        tmp.path(),
        tmp.path(),
        &env.path,
        Duration::from_secs(5),
        |script| {
            let command = env.build_command(tmp.path(), script, None, "");
            assert_no_cache_env(&command);
            command
        },
        std::future::pending(),
    )
    .await
    .unwrap();
    assert!(!tmp.path().join("calls").exists());
}

#[tokio::test]
async fn owner_wrapper_is_kept_without_starting_or_reporting_our_cache() {
    let tmp = tempfile::tempdir().unwrap();
    script(tmp.path(), "sccache", "exit 99");
    let env = VerifyEnvironment::with_cache(
        tmp.path(),
        tmp.path().display().to_string(),
        tmp.path().join("cache"),
        SERVER_PORT,
        true,
    );
    assert!(env.compiler_cache.is_none());
    let mut command = env.build_command(tmp.path(), "printf '%s' \"$RUSTC_WRAPPER\"", None, "");
    assert_no_cache_env(&command);
    command.env("RUSTC_WRAPPER", "owner-wrapper");
    let result = run_verify_command(
        tmp.path(),
        "owner wrapper",
        command,
        Duration::from_secs(5),
        std::future::pending(),
    )
    .await;
    assert_eq!(result.code, Some(0));
    assert_eq!(result.tail, "owner-wrapper");
    assert_eq!(
        CacheRun::begin(env.compiler_cache)
            .await
            .finish("verify")
            .await,
        None
    );
}

#[tokio::test]
async fn gate_stats_are_deltas_and_server_start_uses_its_own_env_and_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("worktree");
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir(&wt).unwrap();
    std::fs::create_dir(&cache_dir).unwrap();
    script(
        tmp.path(),
        "sccache",
        r#"
case "$1" in
  --start-server) printf '%s|%s|%s|%s' "$RUSTC_WRAPPER" "$SCCACHE_DIR" "$SCCACHE_SERVER_PORT" "$PWD" > "$SCCACHE_DIR/start" ;;
  --show-stats) [ "$2" = --stats-format=json ] || exit 1; /bin/cat "$SCCACHE_DIR/stats" ;;
esac"#,
    );
    let env = VerifyEnvironment::with_cache(
        &wt,
        tmp.path().display().to_string(),
        cache_dir.clone(),
        43210,
        false,
    );
    let cache = env.compiler_cache.as_ref().unwrap();
    let stats = |requests: u64, hits: u64| {
        std::fs::write(cache_dir.join("stats"), serde_json::json!({
            "stats":{"compile_requests":requests,"cache_hits":{"counts":{"Rust":hits},"adv_counts":{"rustc":hits}}}
        }).to_string()).unwrap();
    };
    let command = env.build_command(&wt, "true", None, "");
    let values: std::collections::HashMap<_, _> = command.as_std().get_envs().collect();
    assert_eq!(
        values[std::ffi::OsStr::new("RUSTC_WRAPPER")],
        Some(std::ffi::OsStr::new("sccache"))
    );
    assert_eq!(
        values[std::ffi::OsStr::new("SCCACHE_DIR")],
        Some(cache_dir.as_os_str())
    );
    assert_eq!(
        values[std::ffi::OsStr::new("SCCACHE_SERVER_PORT")],
        Some(std::ffi::OsStr::new("43210"))
    );
    let started = std::fs::read_to_string(cache_dir.join("start")).unwrap();
    let fields: Vec<_> = started.split('|').collect();
    assert_eq!(
        &fields[..3],
        &["sccache", cache_dir.to_str().unwrap(), "43210"]
    );
    assert!(!Path::new(fields[3]).starts_with(wt.canonicalize().unwrap()));
    for gate in ["verify", "final checks", "parent gate"] {
        stats(100, 70);
        let run = CacheRun::begin(Some(cache.clone())).await;
        verify(&wt, &env, "true").await;
        stats(152, 111);
        assert_eq!(
            run.finish(gate).await.unwrap(),
            format!("Orchestrator: compiler cache 41/52 hits (79%) in {gate}")
        );
    }
    stats(152, 111);
    let run = CacheRun::begin(Some(cache.clone())).await;
    verify(&wt, &env, "true").await;
    assert_eq!(run.finish("verify").await, None);
}

struct TestServer {
    executable: PathBuf,
    path: String,
    dir: PathBuf,
    port: u16,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Only the server this test started, on its own free port.
        let _ = std::process::Command::new(&self.executable)
            .arg("--stop-server")
            .env("PATH", &self.path)
            .env("SCCACHE_DIR", &self.dir)
            .env("SCCACHE_SERVER_PORT", self.port.to_string())
            .env_remove("SCCACHE_SERVER_UDS")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[tokio::test]
async fn sccache_present_hits_across_two_fresh_verify_builds_with_separate_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let path = VerifyEnvironment::for_worktree(tmp.path()).path;
    if CompilerCache::discover(&path, true, false, tmp.path().join("cache"), SERVER_PORT).is_none()
    {
        eprintln!("SKIP compiler cache: sccache is not installed on orchd's augmented PATH");
        return;
    }
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let first = tmp.path().join("first");
    let second = tmp.path().join("second");
    // Rust cache keys include a crate's cwd. A fixed-location dependency
    // models the registry crates reused by fresh task worktrees.
    let library = tmp.path().join("library");
    std::fs::create_dir_all(library.join("src")).unwrap();
    std::fs::write(
        library.join("Cargo.toml"),
        "[package]\nname = \"orch-cache-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        library.join("src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    )
    .unwrap();
    for wt in [&first, &second] {
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("Cargo.toml"), "[package]\nname = \"orch-cache-demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\norch-cache-lib = { path = \"../library\" }\n[workspace]\n[profile.dev]\nincremental = false\ndebug = 0\n").unwrap();
        std::fs::write(
            wt.join("src/lib.rs"),
            "pub fn answer() -> u32 { orch_cache_lib::answer() }\n",
        )
        .unwrap();
        assert!(!wt.join("target").exists());
    }
    let env =
        VerifyEnvironment::with_cache(&first, path.clone(), tmp.path().join("cache"), port, false);
    let cache = env.compiler_cache.as_ref().unwrap();
    let _server = TestServer {
        executable: std::env::split_paths(&path)
            .map(|p| p.join("sccache"))
            .find(|p| p.is_file())
            .unwrap(),
        path,
        dir: cache.dir.clone(),
        port,
    };
    // The port is held until the server starts: nothing else can take it in between.
    drop(listener);
    let first_run = CacheRun::begin(Some(cache.clone())).await;
    verify(&first, &env, "cargo build --offline").await;
    assert!(
        first_run.finish("verify").await.is_some(),
        "the first build sent compile requests"
    );
    let before = cache
        .snapshot()
        .await
        .expect("our server stats before the second build");
    let second_env =
        VerifyEnvironment::with_cache(&second, env.path.clone(), cache.dir.clone(), port, false);
    verify(&second, &second_env, "cargo build --offline").await;
    let delta = cache
        .snapshot()
        .await
        .expect("server survives verify's group cleanup")
        .delta(before)
        .unwrap();
    assert!(
        delta.hits > 0,
        "second fresh build must hit the compiler cache: {delta:?}"
    );
    assert!(delta.requests > 0);
    for wt in [&first, &second] {
        assert!(
            wt.join("target/debug/liborch_cache_demo.rlib").is_file(),
            "{} owns its target output",
            wt.display()
        );
    }
    assert_ne!(
        first.join("target").canonicalize().unwrap(),
        second.join("target").canonicalize().unwrap()
    );
}

#[test]
fn setup_keeps_per_worktree_build_outputs_with_the_shared_wrapper() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("worktree");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(wt.join(".sushiai")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "sccache", "exit 0");
    std::fs::write(
        wt.join(".sushiai/setup.json"),
        r#"{"setup":"echo \"[$RUSTC_WRAPPER][$CARGO_TARGET_DIR]\" > out.txt"}"#,
    )
    .unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(sushiai_orch::worktree_env::run_setup(
            tmp.path(),
            &wt,
            &path,
            Duration::from_secs(5),
            |script| {
                let mut command = tokio::process::Command::new("/bin/sh");
                command.args(["-c", script]);
                #[cfg(unix)]
                unsafe {
                    command.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
                command
            },
            std::future::pending(),
        ))
        .unwrap();
    let wrapper = std::env::var("RUSTC_WRAPPER").unwrap_or_else(|_| "sccache".into());
    assert_eq!(
        std::fs::read_to_string(wt.join("out.txt")).unwrap().trim(),
        format!("[{wrapper}][]")
    );
}
