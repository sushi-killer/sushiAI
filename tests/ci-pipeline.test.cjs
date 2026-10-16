const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

const root = path.join(__dirname, "..");
const pkg = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
);

function expand(command, seen = new Set()) {
  return command
    .split("&&")
    .map((part) => {
      const p = part.trim();
      const m = /^npm (?:run )?([\w:-]+)$/.exec(p);
      if (m && pkg.scripts[m[1]] && !seen.has(m[1])) {
        return expand(pkg.scripts[m[1]], new Set(seen).add(m[1]));
      }
      return p;
    })
    .join(" && ");
}

const ciChanges = () =>
  import(pathToFileURL(path.join(root, "scripts/ci-changes.mjs")).href);

// A unified diff of one file: `removed` and `added` lines.
const diffOf = (file, added, removed = []) =>
  [
    `diff --git a/${file} b/${file}`,
    `--- a/${file}`,
    `+++ b/${file}`,
    "@@ -1 +1 @@",
    ...removed.map((l) => `-${l}`),
    ...added.map((l) => `+${l}`),
  ].join("\n");

test("a pull request with docs alone skips the builds; Rust jobs need Rust", async () => {
  const { classify } = await ciChanges();
  const scope = (files) => {
    const { code, rust } = classify(files);
    return { code, rust };
  };
  const docs = { code: false, rust: false };
  assert.deepEqual(scope(["README.md", "docs/INSTALL.md"]), docs);
  assert.deepEqual(scope(["docs/releases/unreleased/x.md"]), docs);
  assert.equal(classify(["README.md"]).platform, false);
  // Markdown that tests or tools read is code.
  for (const file of [
    ".agents/skills/ship-pr/SKILL.md",
    "crates/sushiai-orch/README.md",
    "tests/fixtures/a.md",
  ]) {
    assert.deepEqual(scope([file]).code, true, file);
  }
  assert.deepEqual(scope(["README.md", "src/App.tsx"]), {
    code: true,
    rust: false,
  });
  for (const file of [
    "crates/sushiai-orch/src/lib.rs",
    "Cargo.lock",
    ".github/workflows/ci.yml",
    "scripts/nextest-sushiai-bin.mjs",
    "electron/test-fixtures/mcp-home/x.json",
  ]) {
    assert.deepEqual(scope([file]), { code: true, rust: true }, file);
  }
  // No diff to read (a push, a schedule, a new branch): everything runs.
  assert.deepEqual(classify([]), {
    code: true,
    rust: true,
    platform: true,
    reason: "no diff",
  });
});

test("the macOS Rust tests run only for a change that can differ on macOS", async () => {
  const { classify } = await ciChanges();
  const file = "crates/sushiai-orch/src/engine/x.rs";
  const neutral = "fn plan() -> u32 { 1 }";
  const run = (diff, text = neutral, files = [file]) =>
    classify(files, { diff, read: () => text });

  // A change under conditional compilation for macOS.
  const macos = run(diffOf(file, ['#[cfg(target_os = "macos")]', neutral]));
  assert.equal(macos.platform, true);
  assert.match(macos.reason, /platform code/);
  // An edit anywhere in a file that compiles per OS.
  const cfgFile = run(diffOf(file, [neutral]), "#[cfg(unix)]\nfn a() {}");
  assert.equal(cfgFile.platform, true);
  assert.match(cfgFile.reason, /compiles per OS/);
  // Removing platform code is a platform change too.
  assert.equal(
    run(diffOf(file, [], ["libc::killpg(pgid, 9);"])).platform,
    true,
  );
  for (const line of [
    "unsafe { libc::kill(-pgid, 0) };",
    "cmd.process_group(0);",
    'Command::new("sh").args(["-c", "sed -i s/a/b/ f"]);',
    'std::fs::read("/proc/self/stat")',
  ])
    assert.equal(run(diffOf(file, [line])).platform, true, line);

  // A Linux-only change does not need macOS either way it is read: the
  // pattern names the cfg, the OS does not matter. A platform-neutral change
  // runs the Rust tests on ubuntu only.
  const plain = run(diffOf(file, [neutral]));
  assert.deepEqual(
    { rust: plain.rust, platform: plain.platform },
    { rust: true, platform: false },
  );
  assert.equal(run(diffOf(file, ["// a comment about linux"])).platform, false);

  // The build setup: dependencies, toolchain and CI.
  for (const path of [
    "Cargo.lock",
    "crates/sushiai-orch/Cargo.toml",
    "rust-toolchain.toml",
    ".config/nextest.toml",
    ".github/workflows/ci.yml",
    "crates/sushiai-orch/tests/fixtures/fake.sh",
  ])
    assert.equal(classify([path]).platform, true, path);
  // A crate with a platform-only dependency (scripts/ci-platform.json).
  assert.equal(classify(["crates/sushiai-hold/src/lib.rs"]).platform, true);
  // A file whose tests failed only on macOS before.
  const history = classify([
    "crates/sushiai-orch/src/engine/tests/recovery.rs",
  ]);
  assert.equal(history.platform, true);
  assert.match(history.reason, /failed only on macOS/);
  // Web and docs never run the Rust tests, on any OS.
  for (const files of [["src/App.tsx"], ["docs/INSTALL.md"]]) {
    const scope = classify(files, { diff: diffOf(files[0], ["libc::x"]) });
    assert.equal(scope.rust, false, files[0]);
    assert.equal(scope.platform, false, files[0]);
  }
});

test("the Rust matrices shard ubuntu always and macOS only for a platform change", async () => {
  const { matrices, SHARDS } = await ciChanges();
  const plain = matrices({ platform: false });
  assert.deepEqual(plain.build.os, ["ubuntu-latest"]);
  assert.equal(plain.test.include.length, SHARDS["ubuntu-latest"]);
  const all = matrices({ platform: true });
  assert.deepEqual(all.build.os, ["ubuntu-latest", "macos-latest"]);
  const mac = all.test.include.filter((s) => s.os === "macos-latest");
  assert.deepEqual(
    mac.map((s) => `${s.shard}/${s.shards}`),
    Array.from(
      { length: SHARDS["macos-latest"] },
      (_, i) => `${i + 1}/${SHARDS["macos-latest"]}`,
    ),
  );
});

test("scripts/ci-platform.json matches the manifests and the test sources", async () => {
  const { generate, readData } = await import(
    pathToFileURL(path.join(root, "scripts/ci-platform.mjs")).href
  );
  const data = readData();
  assert.deepEqual(
    generate(data),
    data,
    "stale: run node scripts/ci-platform.mjs --write",
  );
  for (const entry of data.macosOnly)
    assert.ok(entry.files.length > 0, `no source defines ${entry.test}`);
});

test("the workspace has no doc tests, so CI runs no cargo test --doc pass", () => {
  // nextest skips doc tests. A ```rust block in a doc comment would be one.
  const found = [];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "target") walk(full);
      } else if (entry.name.endsWith(".rs")) {
        // Fences open and close in turn; only an opening one names a language.
        const fence = /^\s*\/\/[/!]\s*```([\w,]*)/gm;
        const fences = [...fs.readFileSync(full, "utf8").matchAll(fence)];
        for (const [, lang] of fences.filter((_, i) => i % 2 === 0))
          if (["", "rust"].includes(lang))
            found.push(path.relative(root, full));
      }
    }
  };
  walk(path.join(root, "crates"));
  assert.deepEqual(
    [...new Set(found)],
    [],
    "a doc test needs a `cargo test --doc` step in scripts/test-rust.mjs",
  );
});

test("the nextest summary lists FLAKY tests apart from failed ones", async () => {
  const { parse, summary } = await import(
    pathToFileURL(path.join(root, "scripts/nextest-summary.mjs")).href
  );
  const xml = fs.readFileSync(
    path.join(__dirname, "fixtures/nextest-junit.xml"),
    "utf8",
  );
  const cases = parse(xml);
  const status = Object.fromEntries(cases.map((c) => [c.id, c.status]));
  assert.equal(status["ci-probe flaky_once"], "flaky");
  assert.equal(status["ci-probe always_fails"], "failed");
  assert.equal(status["ci-probe passes"], "passed");
  const { text, flaky, failed } = summary(cases, "ubuntu 1/2");
  assert.deepEqual(
    flaky.map((c) => c.id),
    ["ci-probe flaky_once"],
  );
  assert.equal(failed.length, 1);
  assert.match(text, /1 FLAKY/);
  assert.match(text, /`ci-probe flaky_once`/);
});

test("npm run ci still runs every check", async () => {
  assert.equal(pkg.scripts.ci, "node scripts/ci.mjs");
  assert.equal(pkg.scripts["ci:web"], undefined);
  const { lanes } = await import(
    pathToFileURL(path.join(root, "scripts/ci.mjs")).href
  );
  assert.ok(lanes.length >= 2, "at least two lanes");
  const all = lanes
    .flatMap((l) => l.commands)
    .map((c) => expand(c))
    .join(" && ");
  const needles = [
    "vite build",
    "cargo build -p sushiai",
    "node --test",
    "check-conventions.mjs",
    "eslint .",
    "prettier --check",
    "git diff --check",
    "cargo fmt --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "test-rust.mjs",
    "check-crate-deps.mjs",
  ];
  for (const needle of needles) assert.ok(all.includes(needle), needle);
});

test("npm run ci runs only the lanes a change needs", async () => {
  const { lanes, lanesFor } = await import(
    pathToFileURL(path.join(root, "scripts/ci.mjs")).href
  );
  const names = (list) => list.map((l) => l.name);
  const commands = (list) => list.flatMap((l) => l.commands).join(" && ");
  assert.deepEqual(names(lanesFor({ code: true, rust: true })), names(lanes));
  assert.deepEqual(names(lanesFor({ code: false, rust: false })), [
    "static",
    "conventions",
  ]);
  const web = lanesFor({ code: true, rust: false });
  assert.ok(!names(web).includes("rust lint"));
  assert.doesNotMatch(commands(web), /test-rust/);
  assert.match(commands(web), /npm test/);
});

test("runLanes runs lanes side by side and stops a lane at its first failure", async () => {
  const { runLanes } = await import(
    pathToFileURL(path.join(root, "scripts/lib/run-lanes.mjs")).href
  );
  const sleep = "node -e 'setTimeout(()=>{},1500)'";
  const start = Date.now();
  const code = await runLanes(
    [
      { name: "a", commands: [sleep] },
      { name: "b", commands: [sleep] },
    ],
    { quiet: true },
  );
  assert.equal(code, 0);
  assert.ok(Date.now() - start < 2900, "lanes overlapped");

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lanes-"));
  const marker = path.join(dir, "ran");
  const failing = await runLanes(
    [
      {
        name: "x",
        commands: [
          "node -e 'process.exit(3)'",
          `node -e "require('fs').writeFileSync(process.argv[1],'1')" ${marker}`,
        ],
      },
      { name: "y", commands: ["node -e '0'"] },
    ],
    { quiet: true },
  );
  assert.notEqual(failing, 0);
  assert.equal(fs.existsSync(marker), false);
  fs.rmSync(dir, { recursive: true, force: true });
});
