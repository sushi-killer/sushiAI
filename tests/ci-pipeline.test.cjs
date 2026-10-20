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
  // No diff to read (a push, a new branch): everything runs, macOS included.
  const push = classify([]);
  assert.equal(push.all, "no diff");
  assert.equal(push.platform, true);
  assert.equal(push.rustTests, null);
  assert.equal(push.nodeTests, null);
  assert.ok(push.web && push.desktop);
});

test("a change runs the checks that can see it, and the setup runs everything", async () => {
  const { classify } = await ciChanges();
  const pick = (files) => {
    const s = classify(files);
    return {
      all: Boolean(s.all),
      rust: s.rustTests === null ? "all" : s.rustTests,
      node: s.nodeTests === null ? "all" : s.nodeTests,
      web: s.web,
      desktop: s.desktop,
    };
  };
  // A module change: its own tests and the binary's tests that drive it, the
  // node tests that start a real daemon; no daemon, web or desktop runs.
  const orch = pick(["crates/sushiai-orch/src/engine/mod.rs"]);
  assert.equal(orch.all, false);
  assert.ok(orch.rust.includes("sushiai-orch"));
  assert.ok(orch.rust.includes("sushiai-orch::queue"));
  assert.ok(orch.rust.includes("sushiai::orch_module"));
  assert.ok(!orch.rust.includes("sushiai-daemon"));
  assert.ok(!orch.rust.includes("sushiai::lifecycle"));
  assert.ok(!orch.rust.includes("sushiai-routines"));
  assert.ok(orch.node.includes("tests/orchestrator-daemon-real.test.cjs"));
  assert.ok(!orch.node.includes("tests/chat.test.cjs"));
  assert.equal(orch.web, false);
  assert.equal(orch.desktop, false);
  // The protocol reaches every crate that depends on it, and the desktop.
  const protocol = pick(["crates/sushiai-protocol/src/lib.rs"]);
  for (const id of [
    "sushiai-protocol",
    "sushiai-daemon",
    "sushiai-core",
    "sushiai-hold::hold",
    "sushiai-orch::queue",
    "sushiai::lifecycle",
  ])
    assert.ok(protocol.rust.includes(id), id);
  assert.equal(protocol.desktop, true);
  // A file two crates include by #[path] reaches both.
  const shared = pick(["crates/process.rs"]);
  assert.ok(shared.rust.includes("sushiai-daemon"));
  assert.ok(shared.rust.includes("sushiai-orch"));
  // A file a crate reads with include_str! reaches that crate's tests.
  assert.ok(
    pick(["electron/agents.json"]).rust.includes("sushiai-agents"),
    "agents.json",
  );
  // The binary composes every module: all Rust tests.
  assert.equal(pick(["crates/sushiai/src/main.rs"]).rust, "all");
  // A UI change runs no Rust test, every node test, the build and the desktop.
  assert.deepEqual(pick(["src/App.tsx"]), {
    all: false,
    rust: [],
    node: "all",
    web: true,
    desktop: true,
  });
  // A test-only change: every node test, no build, no desktop.
  assert.deepEqual(pick(["tests/chat.test.cjs"]), {
    all: false,
    rust: [],
    node: "all",
    web: false,
    desktop: false,
  });
  // The desktop smoke's own scripts run it.
  assert.equal(pick(["scripts/smoke.mjs"]).desktop, true);
  // Docs alone run nothing.
  assert.deepEqual(pick(["docs/MAP.md"]), {
    all: false,
    rust: [],
    node: [],
    web: false,
    desktop: false,
  });
  // The build and test setup, and a crates file the graph cannot place, run
  // everything.
  for (const file of [
    "Cargo.lock",
    "package-lock.json",
    "rust-toolchain.toml",
    ".config/nextest.toml",
    "tsconfig.json",
    "tests/helpers/real-daemon.cjs",
    "scripts/ci-test-map.json",
    "scripts/lib/rust-graph.mjs",
    "crates/new-thing.txt",
  ])
    assert.equal(pick([file]).all, true, file);
});

test("the required check is a job that waits for every other job", () => {
  // Branch protection names these checks; renaming or dropping the job, or
  // a job outside its `needs`, would let a red job merge.
  const required = JSON.parse(
    fs.readFileSync(path.join(root, ".github/required-checks.json"), "utf8"),
  );
  const yml = fs.readFileSync(
    path.join(root, ".github/workflows/ci.yml"),
    "utf8",
  );
  const body = yml.slice(yml.indexOf("\njobs:\n"));
  const jobs = [...body.matchAll(/^ {2}([\w-]+):\s*$/gm)].map((m) => m[1]);
  const block = (job) => {
    const start = body.indexOf(`\n  ${job}:`);
    const next = body.slice(start + 1).search(/\n {2}[\w-]+:\s*\n/);
    return body.slice(start, next < 0 ? undefined : start + 1 + next);
  };
  assert.deepEqual(required, ["deterministic"]);
  for (const name of required) {
    assert.ok(jobs.includes(name), `no job ${name}`);
    const text = block(name);
    assert.doesNotMatch(text, /^ {4}name:/m, "a name: would rename the check");
    assert.match(text, /if: always\(\)/);
    assert.match(text, /\.result == "skipped"/);
    const needs = (/needs:\s*\[([^\]]*)\]/s.exec(text)?.[1] ?? "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    assert.deepEqual(
      needs.sort(),
      jobs.filter((j) => j !== name).sort(),
      "every other job",
    );
  }
});

test("every Rust test binary has a dependency set, and the binary's tests a mapping", async () => {
  const { testBinaries, crates, CI_CRATES } = await import(
    pathToFileURL(path.join(root, "scripts/lib/rust-graph.mjs")).href
  );
  // A test of the binary without its header would run on every crate
  // change; a new one names the modules it drives in `//! ci-crates:`.
  const dir = path.join(root, "crates/sushiai/tests");
  const names = new Set(crates().map((c) => c.name));
  for (const f of fs.readdirSync(dir).filter((f) => f.endsWith(".rs"))) {
    const line = CI_CRATES.exec(fs.readFileSync(path.join(dir, f), "utf8"));
    assert.ok(line, `${f}: no //! ci-crates: header`);
    for (const w of line[1].trim().split(/\s+/))
      assert.ok(
        w === "host" || w.startsWith("crates-with:") || names.has(w),
        `${f}: unknown crate ${w}`,
      );
  }
  const all = testBinaries();
  for (const b of all) assert.ok(b.deps.has(b.crate), b.id);
  // rust_standards checks every crate that holds a rust-standards.json.
  const standards = all.find((b) => b.id === "sushiai::rust_standards");
  for (const c of crates())
    if (fs.existsSync(path.join(root, c.dir, "rust-standards.json")))
      assert.ok(standards.deps.has(c.name), c.name);
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
  // A crate with a platform-only dependency (scripts/ci-platform.mjs).
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

test("the platform data comes from the manifests and the marked test files", async () => {
  const { readData, MACOS_ONLY } = await import(
    pathToFileURL(path.join(root, "scripts/ci-platform.mjs")).href
  );
  const data = readData();
  assert.ok(data.crates.includes("crates/sushiai-hold"));
  for (const file of data.macosOnly) {
    const text = fs.readFileSync(path.join(root, file), "utf8");
    // Each marker names the CI run that showed the macOS-only failure.
    for (const line of text.split("\n").filter((l) => MACOS_ONLY.test(l)))
      assert.match(line, /run \d+/, `${file}: ${line.trim()}`);
  }
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

test("a flaky test on main opens one issue, and each further flake comments on it", async () => {
  const { plan, titleOf, LABEL } = await import(
    pathToFileURL(path.join(root, "scripts/flaky-report.mjs")).href
  );
  const cases = [
    { id: "a x", status: "flaky" },
    { id: "b y", status: "flaky" },
    { id: "c z", status: "failed" },
    { id: "d w", status: "passed" },
  ];
  const calls = plan(cases, [{ number: 7, title: titleOf("b y") }], "RUN");
  assert.equal(calls.length, 2, "failed and passed tests are not flakes");
  assert.deepEqual(calls[0].slice(0, 6), [
    "issue",
    "create",
    "--title",
    titleOf("a x"),
    "--label",
    LABEL,
  ]);
  assert.deepEqual(calls[1].slice(0, 3), ["issue", "comment", "7"]);
  assert.match(calls[1][4], /RUN/);
});

test("a quarantined test has an owner, an issue and an expiry not yet passed", async () => {
  const { readQuarantine, rustFilter } = await ciChanges();
  const today = new Date().toISOString().slice(0, 10);
  for (const q of readQuarantine()) {
    for (const key of ["binary", "test", "owner", "issue", "expires"])
      assert.ok(q[key], `${q.test}: ${key}`);
    assert.match(q.expires, /^\d{4}-\d{2}-\d{2}$/);
    assert.ok(
      q.expires >= today,
      `${q.test}: quarantine expired ${q.expires}; fix the test or renew it with its owner`,
    );
  }
  const q = [{ binary: "b::t", test: "m::x" }];
  assert.equal(rustFilter({ rustTests: null }, []), "");
  assert.equal(
    rustFilter({ rustTests: null }, q),
    "not (binary_id(=b::t) & test(=m::x))",
  );
  assert.equal(
    rustFilter({ rustTests: ["a"] }, q),
    "(binary_id(=a)) & not (binary_id(=b::t) & test(=m::x))",
  );
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
  const { classify } = await ciChanges();
  const names = (list) => list.map((l) => l.name);
  const commands = (list) => list.flatMap((l) => l.commands).join(" && ");
  assert.deepEqual(lanesFor(classify([])), lanes);
  assert.deepEqual(names(lanesFor(classify(["README.md"]))), [
    "static",
    "conventions",
  ]);
  const web = lanesFor(classify(["src/App.tsx"]));
  assert.ok(!names(web).includes("rust lint"));
  assert.doesNotMatch(commands(web), /test-rust/);
  assert.match(commands(web), /npm run build && .*npm test/);
  const orch = commands(
    lanesFor(classify(["crates/sushiai-orch/src/engine/mod.rs"])),
  );
  assert.doesNotMatch(orch, /npm run build|npm test/);
  assert.match(orch, /node --test tests\/\S+/);
  assert.match(orch, /test-rust\.mjs --affected/);
  assert.match(orch, /lint:rust/);
});

test("runLanes runs lanes side by side and stops a lane at its first failure", async () => {
  const { runLanes } = await import(
    pathToFileURL(path.join(root, "scripts/lib/run-lanes.mjs")).href
  );
  // Each lane announces itself, then waits for the other's announcement. Only
  // lanes that run side by side can both finish; one after the other, the
  // first gives up at its own limit and the run fails.
  const rendezvous = fs.mkdtempSync(path.join(os.tmpdir(), "lanes-meet-"));
  const meet = path.join(rendezvous, "meet.cjs");
  fs.writeFileSync(
    meet,
    `const fs = require("fs");
const [mine, theirs] = process.argv.slice(2);
fs.writeFileSync(mine, "1");
const limit = Date.now() + 30000;
const timer = setInterval(() => {
  if (fs.existsSync(theirs)) process.exit(0);
  if (Date.now() > limit) process.exit(1);
}, 10);
`,
  );
  const mark = (name) => path.join(rendezvous, name);
  let code;
  try {
    code = await runLanes(
      [
        { name: "a", commands: [`node ${meet} ${mark("a")} ${mark("b")}`] },
        { name: "b", commands: [`node ${meet} ${mark("b")} ${mark("a")}`] },
      ],
      { quiet: true },
    );
  } finally {
    fs.rmSync(rendezvous, { recursive: true, force: true });
  }
  assert.equal(code, 0, "lanes overlapped");

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
