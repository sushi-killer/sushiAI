const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");
const {
  ARTIFACTS_MANIFEST,
} = require("../electron/extensions/builtin-artifacts.cjs");
const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");
const { configureArtifactsSkill } = require("../electron/artifacts-skill.cjs");
const {
  writeSkill,
  remoteInstallScript: buildRemoteInstallScript,
  syncBuiltinSkillsOnHost,
  syncLocalBuiltinSkills,
  installBuiltinSkillsInto,
} = require("../electron/extensions/builtin-skills.cjs");

const NAME = "sushiai-artifacts";
configureArtifactsSkill({ isEnabled: () => true });
const installLocalArtifactsSkill = (home, text, accounts = []) =>
  writeSkill(
    NAME,
    [path.join(home, ".claude"), path.join(home, ".codex"), ...accounts],
    text,
  );
const remoteInstallScript = (text) => buildRemoteInstallScript(NAME, text);
const installRemoteArtifactsSkill = (connections, endpoint, env) =>
  syncBuiltinSkillsOnHost(connections, endpoint, env);

const TEXT =
  "name: sushiai-artifacts\nSkill \"quoted\" $HOME `tick` 'single'\nline two\n";

function tempHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "artifacts-skill-"));
}

test("the artifacts manifest validates as a builtin", () => {
  const manifest = validateExtensionManifest(
    structuredClone(ARTIFACTS_MANIFEST),
  );
  assert.equal(manifest.id, "builtin.artifacts");
  assert.deepEqual(manifest.contributions.surfaces[0].allowedHosts, [
    "workspace.pane",
  ]);
  assert.equal(manifest.contributions.surfaces[0].instancePolicy, "multiple");
});

test("a non-builtin copy with the core view is rejected", () => {
  const copy = structuredClone(ARTIFACTS_MANIFEST);
  copy.id = "acme.artifacts";
  copy.source = { kind: "local" };
  assert.throws(() => validateExtensionManifest(copy), /core views|source/i);
});

test("the artifacts builtin can be turned off", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "artifacts-manager-"));
  try {
    const manager = new ExtensionManager({
      dataDir: dir,
      builtins: [ARTIFACTS_MANIFEST],
      localDir: path.join(dir, "local"),
    });
    await manager.init?.();
    assert.equal(manager.isEnabled("builtin.artifacts"), true);
    await manager.setEnabled("builtin.artifacts", false);
    assert.equal(manager.isEnabled("builtin.artifacts"), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("local install writes Claude Code, Codex and every Codex account home, idempotently", () => {
  const home = tempHome();
  try {
    const account = path.join(home, "accounts", "a1");
    const first = installLocalArtifactsSkill(home, TEXT, [account]);
    const files = [
      path.join(home, ".claude/skills/sushiai-artifacts/SKILL.md"),
      path.join(home, ".codex/skills/sushiai-artifacts/SKILL.md"),
      path.join(account, "skills/sushiai-artifacts/SKILL.md"),
    ];
    assert.deepEqual(first, files);
    for (const file of files) assert.equal(fs.readFileSync(file, "utf8"), TEXT);
    assert.deepEqual(installLocalArtifactsSkill(home, TEXT, [account]), []);
    assert.deepEqual(
      installLocalArtifactsSkill(home, TEXT + "x", [account]),
      files,
    );
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the remote script round-trips the exact content through a fake exec", async () => {
  const home = tempHome();
  const calls = [];
  try {
    fs.mkdirSync(path.join(home, ".codex"));
    const exec = async (endpoint, command, { input }) => {
      calls.push({ endpoint, command });
      return execFileSync("/bin/sh", ["-s"], {
        input,
        // The host's own config folders must not leak in from this machine.
        env: {
          ...process.env,
          HOME: home,
          CODEX_HOME: "",
          CLAUDE_CONFIG_DIR: "",
        },
      }).toString();
    };
    const saved = process.env.SUSHIAI_TEST_WINDOW;
    delete process.env.SUSHIAI_TEST_WINDOW;
    try {
      const ok = await installRemoteArtifactsSkill(
        { exec },
        "ssh:user@devbox",
        {},
      );
      assert.equal(ok, true);
    } finally {
      if (saved !== undefined) process.env.SUSHIAI_TEST_WINDOW = saved;
    }
    assert.equal(calls[0].endpoint, "ssh:user@devbox");
    const file = path.join(home, ".codex/skills/sushiai-artifacts/SKILL.md");
    assert.equal(
      fs.readFileSync(file, "utf8"),
      fs.readFileSync(
        path.join(__dirname, "../electron/extensions/artifacts-skill.md"),
        "utf8",
      ),
    );
    assert.ok(
      fs.existsSync(
        path.join(home, ".claude/skills/sushiai-artifacts/SKILL.md"),
      ),
    );
    assert.ok(
      fs.existsSync(
        path.join(home, ".agents/skills/sushiai-artifacts/SKILL.md"),
      ),
    );
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the remote script decodes to the exact text, hostile characters included", () => {
  const home = tempHome();
  try {
    fs.mkdirSync(path.join(home, ".claude"));
    execFileSync("/bin/sh", ["-s"], {
      input: remoteInstallScript(TEXT),
      env: { ...process.env, HOME: home },
    });
    assert.equal(
      fs.readFileSync(
        path.join(home, ".claude/skills/sushiai-artifacts/SKILL.md"),
        "utf8",
      ),
      TEXT,
    );
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("a failing host exec never throws", async () => {
  const saved = process.env.SUSHIAI_TEST_WINDOW;
  delete process.env.SUSHIAI_TEST_WINDOW;
  const warn = console.warn;
  console.warn = () => {};
  try {
    const exec = async () => {
      throw new Error("unreachable 192.0.2.7");
    };
    assert.equal(
      await installRemoteArtifactsSkill({ exec }, "ssh:user@devbox", {}),
      false,
    );
  } finally {
    console.warn = warn;
    if (saved !== undefined) process.env.SUSHIAI_TEST_WINDOW = saved;
  }
});

function withRealEnv(fn) {
  const saved = process.env.SUSHIAI_TEST_WINDOW;
  delete process.env.SUSHIAI_TEST_WINDOW;
  try {
    return fn();
  } finally {
    if (saved !== undefined) process.env.SUSHIAI_TEST_WINDOW = saved;
  }
}

test("a disabled extension installs nothing, locally or on a host", async () => {
  const home = tempHome();
  try {
    configureArtifactsSkill({ isEnabled: () => false });
    await withRealEnv(async () => {
      assert.deepEqual(syncLocalBuiltinSkills(home), []);
      installBuiltinSkillsInto(path.join(home, "acct"));
      const scripts = [];
      await syncBuiltinSkillsOnHost(
        { exec: async (e, c, { input }) => scripts.push(input) },
        "ssh:user@devbox",
      );
      assert.equal(scripts.length, 1);
      assert.doesNotMatch(scripts[0], /base64/);
      assert.match(scripts[0], /rm -f/);
    });
    assert.deepEqual(fs.readdirSync(home), []);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("turning the extension off removes only our skill, everywhere; on installs it again", () => {
  const home = tempHome();
  try {
    const accounts = path.join(home, "codex-accounts");
    const account = path.join(accounts, "a1");
    fs.mkdirSync(account, { recursive: true });
    let enabled = true;
    let listener;
    configureArtifactsSkill({
      isEnabled: () => enabled,
      subscribe: (fn) => (listener = fn),
      home,
      codexAccountsDir: accounts,
    });
    withRealEnv(() => {
      syncLocalBuiltinSkills(home, undefined, accounts);
      const ours = [
        path.join(home, ".claude/skills", NAME, "SKILL.md"),
        path.join(home, ".codex/skills", NAME, "SKILL.md"),
        path.join(account, "skills", NAME, "SKILL.md"),
      ];
      for (const file of ours) assert.ok(fs.existsSync(file), file);
      // A foreign folder of the same name stays.
      const foreign = path.join(account, "skills", NAME, "SKILL.md");
      const mine = fs.readFileSync(foreign, "utf8");
      fs.writeFileSync(foreign, "---\nname: someone-else\n---\n");
      enabled = false;
      listener("builtin.artifacts", false);
      assert.equal(fs.existsSync(ours[0]), false);
      assert.equal(fs.existsSync(path.dirname(ours[0])), false);
      assert.equal(fs.existsSync(ours[1]), false);
      assert.equal(fs.existsSync(foreign), true);
      // Another extension's change does nothing.
      enabled = true;
      listener("builtin.other", true);
      assert.equal(fs.existsSync(ours[0]), false);
      listener("builtin.artifacts", true);
      assert.ok(fs.existsSync(ours[0]));
      assert.equal(
        fs.readFileSync(foreign, "utf8").includes("someone-else"),
        true,
      );
      assert.ok(mine.includes(`name: ${NAME}`));
    });
  } finally {
    configureArtifactsSkill({ isEnabled: () => true });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the remote removal script deletes ours and leaves a foreign skill", async () => {
  const home = tempHome();
  try {
    const own = path.join(home, ".claude/skills", NAME);
    const foreign = path.join(home, ".codex/skills", NAME);
    fs.mkdirSync(own, { recursive: true });
    fs.mkdirSync(foreign, { recursive: true });
    fs.writeFileSync(path.join(own, "SKILL.md"), `---\nname: ${NAME}\n---\n`);
    fs.writeFileSync(path.join(foreign, "SKILL.md"), "---\nname: other\n---\n");
    configureArtifactsSkill({ isEnabled: () => false });
    const exec = async (e, c, { input }) =>
      execFileSync("/bin/sh", ["-s"], {
        input,
        env: { ...process.env, HOME: home },
      }).toString();
    await withRealEnv(() => syncBuiltinSkillsOnHost({ exec }, "ssh:u@h"));
    assert.equal(fs.existsSync(own), false);
    assert.equal(fs.existsSync(path.join(foreign, "SKILL.md")), true);
  } finally {
    configureArtifactsSkill({ isEnabled: () => true });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the skill goes into the shared agents folder and custom config folders", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const skills = require("../electron/extensions/builtin-skills.cjs");
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "skills-home-"));
  const custom = fs.mkdtempSync(path.join(os.tmpdir(), "skills-codex-"));
  skills.registerBuiltinSkill({
    extensionId: "builtin.test-shared",
    name: "test-shared-skill",
    text: "---\nname: test-shared-skill\ndescription: x\n---\n",
    isEnabled: () => true,
  });
  skills.syncLocalBuiltinSkills(home, { CODEX_HOME: custom }, undefined);
  for (const dir of [
    path.join(home, ".claude"),
    path.join(home, ".codex"),
    path.join(home, ".agents"),
    custom,
  ])
    assert.ok(
      fs.existsSync(path.join(dir, "skills", "test-shared-skill", "SKILL.md")),
      dir,
    );
  const script = skills.remoteInstallScript("test-shared-skill", "x");
  assert.match(script, /CODEX_HOME:-\$HOME\/\.codex/);
  assert.match(script, /\$HOME\/\.agents/);
});

test("the skill teaches sushiai open with an absolute path and no Herdr command", () => {
  const text = fs.readFileSync(
    path.join(__dirname, "..", "electron", "extensions", "artifacts-skill.md"),
    "utf8",
  );
  assert.match(text, /sushiai open artifacts\/preview "\$\(pwd\)\/artifacts\//);
  assert.match(text, /SUSHIAI_SESSION_ID/);
  assert.doesNotMatch(text, /herdr/i);
});
